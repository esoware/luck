use luck_token::LuaVersion;

use crate::rng::Rng;

const MAX_STMT_DEPTH: usize = 4;

/// Module-scope locals to stop at. PUC-Rio Lua rejects a function with more
/// than 200 active locals, and the blocks nested below module scope add
/// their own on top.
const MAX_MODULE_LOCALS: usize = 150;

/// A small pool of stems forces shadowing, the bug class a flat name-based
/// analysis gets wrong.
const NAME_STEMS: [&str; 6] = ["alpha", "beta", "gamma", "delta", "value", "item"];

/// Value categories tracked so generated operations are type-safe at
/// runtime (no `"a" + 1`, no calling a number).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Number,
    String,
    Boolean,
    Table,
}

#[derive(Debug)]
struct Binding {
    name: String,
    kind: ValueKind,
    /// A loop's counter or control variable, which a generated assignment
    /// must leave alone so the loop stays bounded and, on 5.5, compiles.
    is_read_only: bool,
}

#[derive(Debug)]
struct RuntimeGenerator {
    rng: Rng,
    version: LuaVersion,
    output: String,
    scopes: Vec<Vec<Binding>>,
    next_id: usize,
    module_locals: usize,
}

impl RuntimeGenerator {
    fn new(seed: u64, version: LuaVersion) -> Self {
        Self {
            rng: Rng::new(seed),
            version,
            output: String::new(),
            scopes: vec![Vec::new()],
            next_id: 0,
            module_locals: 0,
        }
    }

    fn program(mut self, statement_budget: usize) -> String {
        for _ in 0..statement_budget {
            self.stmt();
        }
        // Print every top-level binding at the end so transforms that
        // wrongly eliminate or reorder assignments change stdout. A binding a
        // later one hides can't be named any more.
        let bindings = self.scopes.pop().unwrap_or_default();
        for (i, Binding { name, kind, .. }) in bindings.iter().enumerate() {
            if bindings.iter().skip(i.saturating_add(1)).any(|later| later.name == *name) {
                continue;
            }
            match kind {
                ValueKind::Table => {
                    self.line(&format!("print(\"{name}\", #{name})"));
                }
                ValueKind::Number | ValueKind::String | ValueKind::Boolean => {
                    self.line(&format!("print(\"{name}\", {name})"));
                }
            }
        }
        self.output
    }

    fn line(&mut self, text: &str) {
        if self.scopes.len() == 1 && text.starts_with("local ") {
            self.module_locals = self.module_locals.saturating_add(1);
        }
        // The module scope adds no indentation.
        for _ in 1..self.scopes.len() {
            self.output.push('\t');
        }
        self.output.push_str(text);
        self.output.push('\n');
    }

    /// A stem and that stem numbered, which never repeats.
    fn numbered_stem(&mut self) -> (&'static str, String) {
        let stem = self.rng.pick(&NAME_STEMS);
        self.next_id = self.next_id.saturating_add(1);
        (stem, format!("{stem}{}", self.next_id))
    }

    /// A numbered stem, or 30% of the time the bare stem, which shadows any
    /// earlier binding of it.
    fn fresh_name(&mut self) -> String {
        let (stem, numbered) = self.numbered_stem();
        if self.rng.chance(30) { stem.to_owned() } else { numbered }
    }

    fn visible_name(&mut self, kind: ValueKind) -> Option<String> {
        self.pick_name(kind, |_| true)
    }

    /// Like [`Self::visible_name`], keeping to bindings a generated
    /// assignment may overwrite.
    fn assignable_name(&mut self, kind: ValueKind) -> Option<String> {
        self.pick_name(kind, |binding| !binding.is_read_only)
    }

    fn pick_name(&mut self, kind: ValueKind, is_offered: fn(&Binding) -> bool) -> Option<String> {
        // A name refers to its innermost binding, which may be a different
        // kind, so a binding shadowed by another kind isn't offered.
        let candidates: Vec<&str> = self
            .scopes
            .iter()
            .flatten()
            .filter(|binding| {
                binding.kind == kind
                    && is_offered(binding)
                    && self.innermost_kind(&binding.name) == Some(kind)
            })
            .map(|binding| binding.name.as_str())
            .collect();
        if candidates.is_empty() {
            return None;
        }
        Some(self.rng.pick(&candidates).to_owned())
    }

    fn innermost_kind(&self, name: &str) -> Option<ValueKind> {
        self.scopes
            .iter()
            .flatten()
            .rev()
            .find(|binding| binding.name == name)
            .map(|binding| binding.kind)
    }

    fn declare(&mut self, name: String, kind: ValueKind) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(Binding { name, kind, is_read_only: false });
        }
    }

    fn declare_read_only(&mut self, name: String, kind: ValueKind) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(Binding { name, kind, is_read_only: true });
        }
    }

    fn number_expr(&mut self, depth: usize) -> String {
        let Some(inner_depth) = depth.checked_sub(1) else {
            return self.number_leaf();
        };
        match self.rng.below(6) {
            0 => self.number_leaf(),
            1 => {
                let lhs = self.number_expr(inner_depth);
                let rhs = self.number_expr(inner_depth);
                let op = self.rng.pick(&["+", "-", "*"]);
                format!("({lhs} {op} {rhs})")
            }
            2 => {
                // Division by a non-zero literal only.
                let lhs = self.number_expr(inner_depth);
                let denominator = self.rng.one_through(9);
                format!("({lhs} / {denominator})")
            }
            3 if self.version.has_floor_div() => {
                let lhs = self.number_expr(inner_depth);
                let denominator = self.rng.one_through(9);
                format!("({lhs} // {denominator})")
            }
            4 => {
                let inner = self.number_expr(inner_depth);
                format!("(-({inner}))")
            }
            _ => self.visible_name(ValueKind::Number).unwrap_or_else(|| self.number_leaf()),
        }
    }

    fn number_leaf(&mut self) -> String {
        match self.rng.below(5) {
            0 => self.rng.below(1000).to_string(),
            1 => format!("{}.5", self.rng.below(100)),
            2 => format!("0x{:x}", self.rng.below(4096)),
            3 => self.rng.below(10).to_string(),
            _ => format!("{}e{}", self.rng.one_through(9), self.rng.below(4)),
        }
    }

    fn string_expr(&mut self, depth: usize) -> String {
        if let Some(inner_depth) = depth.checked_sub(1)
            && self.rng.chance(40)
        {
            let lhs = self.string_expr(inner_depth);
            let rhs = if self.rng.chance(50) {
                self.string_expr(inner_depth)
            } else {
                // Numbers coerce in concat.
                self.number_expr(0)
            };
            return format!("({lhs} .. {rhs})");
        }
        let literal = match self.rng.below(6) {
            0 => "\"plain\"",
            1 => "'single'",
            2 => "\"esc\\ttab\\n\"",
            3 => "\"d\\101c\"", // decimal escape
            4 => "[[long bracket]]",
            _ => "\"uni-h\u{e9}llo\"",
        };
        literal.to_owned()
    }

    fn boolean_expr(&mut self, depth: usize) -> String {
        if let Some(inner_depth) = depth.checked_sub(1)
            && self.rng.chance(50)
        {
            let lhs = self.number_expr(inner_depth);
            let rhs = self.number_expr(inner_depth);
            let op = self.rng.pick(&["==", "~=", "<", "<=", ">", ">="]);
            return format!("({lhs} {op} {rhs})");
        }
        match self.rng.below(3) {
            0 => "true".to_owned(),
            1 => "false".to_owned(),
            _ => self.visible_name(ValueKind::Boolean).unwrap_or_else(|| "true".to_owned()),
        }
    }

    fn table_expr(&mut self) -> String {
        let array_field_count = self.rng.one_through(4);
        let mut fields = Vec::new();
        for _ in 0..array_field_count {
            fields.push(self.number_expr(1));
        }
        if self.rng.chance(40) {
            let key_value = self.number_expr(0);
            fields.push(format!("named = {key_value}"));
        }
        format!("{{ {} }}", fields.join(", "))
    }

    fn expr_of(&mut self, kind: ValueKind, depth: usize) -> String {
        match kind {
            ValueKind::Number => self.number_expr(depth),
            ValueKind::String => self.string_expr(depth),
            ValueKind::Boolean => self.boolean_expr(depth),
            ValueKind::Table => self.table_expr(),
        }
    }

    fn random_kind(&mut self) -> ValueKind {
        match self.rng.below(10) {
            0..=4 => ValueKind::Number,
            5..=6 => ValueKind::String,
            7..=8 => ValueKind::Boolean,
            _ => ValueKind::Table,
        }
    }

    fn stmt(&mut self) {
        if self.scopes.len() == 1 && self.module_locals >= MAX_MODULE_LOCALS {
            self.print_stmt();
            return;
        }
        // The module scope isn't a nesting level.
        let can_nest = self.scopes.len() <= MAX_STMT_DEPTH;
        match self.rng.below(12) {
            3 => self.assignment(),
            4 if can_nest => self.if_stmt(),
            5 if can_nest => self.numeric_for(),
            6 if can_nest => self.while_stmt(),
            7 if can_nest => self.do_block(),
            8 if can_nest => self.local_function(),
            9 if can_nest => self.repeat_stmt(),
            10 => self.print_stmt(),
            _ => self.local_declaration(),
        }
    }

    fn local_declaration(&mut self) {
        let kind = self.random_kind();
        let name = self.fresh_name();
        let value = self.expr_of(kind, 2);
        self.line(&format!("local {name} = {value}"));
        self.declare(name, kind);
    }

    fn assignment(&mut self) {
        if let Some(name) = self.assignable_name(ValueKind::Number) {
            let value = self.number_expr(2);
            if self.version.has_compound_assignment() && self.rng.chance(30) {
                self.line(&format!("{name} += {value}"));
            } else {
                self.line(&format!("{name} = {value}"));
            }
        } else {
            self.local_declaration();
        }
    }

    fn print_stmt(&mut self) {
        let kind = self.random_kind();
        let value = match (kind, self.visible_name(kind)) {
            // A table prints as its address, which differs between runs.
            (ValueKind::Table, Some(name)) => format!("#{name}"),
            (ValueKind::Table, None) => self.number_expr(1),
            (ValueKind::Number | ValueKind::String | ValueKind::Boolean, Some(name)) => name,
            (ValueKind::Number | ValueKind::String | ValueKind::Boolean, None) => {
                self.expr_of(kind, 1)
            }
        };
        self.line(&format!("print({value})"));
    }

    fn block<F>(&mut self, fill: F)
    where
        F: FnOnce(&mut Self),
    {
        self.scopes.push(Vec::new());
        fill(self);
        self.scopes.pop();
    }

    fn small_body(&mut self) {
        let statement_count = self.rng.one_through(3);
        for _ in 0..statement_count {
            self.stmt();
        }
    }

    fn if_stmt(&mut self) {
        let condition = self.boolean_expr(2);
        self.line(&format!("if {condition} then"));
        self.block(Self::small_body);
        if self.rng.chance(40) {
            let elseif_condition = self.boolean_expr(1);
            self.line(&format!("elseif {elseif_condition} then"));
            self.block(Self::small_body);
        }
        if self.rng.chance(50) {
            self.line("else");
            self.block(Self::small_body);
        }
        self.line("end");
    }

    fn numeric_for(&mut self) {
        // 5.5 control variables are read-only: keep the name globally
        // unique (a bare-stem shadow would make later writes resolve to
        // the loop var) and never offer it as an assignment target.
        let is_read_only = self.version.has_const_for_variables();
        let loop_variable = if is_read_only { self.numbered_stem().1 } else { self.fresh_name() };
        let stop = self.rng.one_through(4);
        self.line(&format!("for {loop_variable} = 1, {stop} do"));
        self.block(|generator| {
            if is_read_only {
                generator.declare_read_only(loop_variable, ValueKind::Number);
            } else {
                generator.declare(loop_variable, ValueKind::Number);
            }
            generator.small_body();
        });
        self.line("end");
    }

    /// Declares a loop's numbered counter, which nothing can shadow and no
    /// generated assignment touches, so the loop stays bounded.
    fn loop_counter(&mut self) -> String {
        let counter = self.numbered_stem().1;
        self.line(&format!("local {counter} = 0"));
        self.declare_read_only(counter.clone(), ValueKind::Number);
        counter
    }

    fn while_stmt(&mut self) {
        let counter = self.loop_counter();
        let limit = self.rng.one_through(4);
        self.line(&format!("while {counter} < {limit} do"));
        self.block(|generator| {
            generator.line(&format!("{counter} = {counter} + 1"));
            generator.small_body();
        });
        self.line("end");
    }

    fn repeat_stmt(&mut self) {
        let counter = self.loop_counter();
        let limit = self.rng.one_through(3);
        self.line("repeat");
        self.block(|generator| {
            generator.line(&format!("{counter} = {counter} + 1"));
            generator.small_body();
        });
        self.line(&format!("until {counter} >= {limit}"));
    }

    fn do_block(&mut self) {
        self.line("do");
        self.block(Self::small_body);
        self.line("end");
    }

    fn local_function(&mut self) {
        // Numbered, so the untracked function name takes over no tracked
        // binding and no local hides the parameter from the closing `return`.
        let name = self.numbered_stem().1;
        let param = self.numbered_stem().1;
        self.line(&format!("local function {name}({param})"));
        self.block(|generator| {
            generator.declare(param.clone(), ValueKind::Number);
            generator.small_body();
            let result = generator.number_expr(1);
            generator.line(&format!("return {param} + {result}"));
        });
        self.line("end");
        // The function isn't tracked as a binding, so call it right away with
        // a literal and print the result to make its body observable.
        let arg = self.rng.below(100);
        self.line(&format!("print({name}({arg}))"));
    }
}

#[must_use]
pub fn generate_runtime_safe(seed: u64, version: LuaVersion, statement_budget: usize) -> String {
    RuntimeGenerator::new(seed, version).program(statement_budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_generates_the_same_program() {
        let first = generate_runtime_safe(42, LuaVersion::Lua54, 30);
        let second = generate_runtime_safe(42, LuaVersion::Lua54, 30);
        assert_eq!(first, second);
    }

    #[test]
    fn different_seeds_generate_different_programs() {
        let first = generate_runtime_safe(1, LuaVersion::Lua54, 30);
        let second = generate_runtime_safe(2, LuaVersion::Lua54, 30);
        assert_ne!(first, second);
    }
}
