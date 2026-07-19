//! Deterministic random-program generator for property tests,
//! the interpreter-differential harness, and bench corpora.
//!
//! Programs are generated from a seed and are:
//! - syntactically valid for the requested `LuaVersion`;
//! - runtime-safe: every variable is initialized, arithmetic operands are
//!   numbers, loops are bounded, no library calls with observable
//!   nondeterminism (`pairs` order, time, GC);
//! - observable: values are `print`ed so behavior differences show up on
//!   stdout when original and transformed programs are executed.

use luck_token::LuaVersion;

mod full;

pub use full::{FullGenerator, generate_full};

/// xorshift64* - deterministic, no external dependency, stable across
/// platforms. Speed and statistical quality are irrelevant here; only
/// reproducibility matters.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// Value categories tracked so generated operations are type-safe at
/// runtime (no `"a" + 1`, no calling a number).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Num,
    Str,
    Bool,
    Table,
}

struct Binding {
    name: String,
    kind: Kind,
}

pub struct Generator {
    rng: Rng,
    version: LuaVersion,
    out: String,
    indent: usize,
    scopes: Vec<Vec<Binding>>,
    next_id: usize,
    depth: usize,
}

const MAX_DEPTH: usize = 4;

impl Generator {
    pub fn new(seed: u64, version: LuaVersion) -> Self {
        Self {
            rng: Rng::new(seed),
            version,
            out: String::new(),
            indent: 0,
            scopes: vec![Vec::new()],
            next_id: 0,
            depth: 0,
        }
    }

    /// Generate a complete program with roughly `statement_budget`
    /// top-level statements.
    pub fn program(mut self, statement_budget: usize) -> String {
        for _ in 0..statement_budget {
            self.statement();
        }
        // Print every top-level binding at the end so transforms that
        // wrongly eliminate or reorder assignments change stdout.
        let names: Vec<(String, Kind)> = self.scopes[0]
            .iter()
            .map(|binding| (binding.name.clone(), binding.kind))
            .collect();
        for (name, kind) in names {
            match kind {
                Kind::Table => {
                    self.line(&format!("print(\"{name}\", #{name})"));
                }
                _ => {
                    self.line(&format!("print(\"{name}\", {name})"));
                }
            }
        }
        self.out
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push('\t');
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn fresh_name(&mut self) -> String {
        // Small pool of stems forces shadowing, the bug class that
        // killed the minifier's flat analyses.
        const STEMS: [&str; 6] = ["alpha", "beta", "gamma", "delta", "value", "item"];
        let stem = STEMS[self.rng.below(STEMS.len())];
        self.next_id += 1;
        if self.rng.chance(30) {
            // Deliberate shadow: reuse a bare stem.
            stem.to_string()
        } else {
            format!("{stem}{}", self.next_id)
        }
    }

    fn visible(&mut self, kind: Kind) -> Option<String> {
        let mut candidates = Vec::new();
        for scope in &self.scopes {
            for binding in scope {
                if binding.kind == kind {
                    candidates.push(binding.name.clone());
                }
            }
        }
        if candidates.is_empty() {
            return None;
        }
        // Later declarations shadow earlier same-named ones; picking any
        // candidate name is still valid because shadowing keeps the kind
        // constraint only if kinds match. Filter to names whose *last*
        // (innermost visible) binding has the right kind.
        let mut valid: Vec<String> = candidates;
        valid.retain(|name| self.last_kind_of(name) == Some(kind));
        if valid.is_empty() {
            return None;
        }
        let idx = self.rng.below(valid.len());
        Some(valid.swap_remove(idx))
    }

    fn last_kind_of(&self, name: &str) -> Option<Kind> {
        for scope in self.scopes.iter().rev() {
            for binding in scope.iter().rev() {
                if binding.name == name {
                    return Some(binding.kind);
                }
            }
        }
        None
    }

    fn declare(&mut self, name: String, kind: Kind) {
        self.scopes
            .last_mut()
            .expect("scope stack never empty")
            .push(Binding { name, kind });
    }

    fn num_expr(&mut self, depth: usize) -> String {
        if depth == 0 {
            return self.num_leaf();
        }
        match self.rng.below(6) {
            0 => self.num_leaf(),
            1 => {
                let lhs = self.num_expr(depth - 1);
                let rhs = self.num_expr(depth - 1);
                let op = ["+", "-", "*"][self.rng.below(3)];
                format!("({lhs} {op} {rhs})")
            }
            2 => {
                // Division by a non-zero literal only.
                let lhs = self.num_expr(depth - 1);
                let denominator = 1 + self.rng.below(9);
                format!("({lhs} / {denominator})")
            }
            3 if self.version.has_floor_div() => {
                let lhs = self.num_expr(depth - 1);
                let denominator = 1 + self.rng.below(9);
                format!("({lhs} // {denominator})")
            }
            4 => {
                let inner = self.num_expr(depth - 1);
                format!("(-({inner}))")
            }
            _ => match self.visible(Kind::Num) {
                Some(name) => name,
                None => self.num_leaf(),
            },
        }
    }

    fn num_leaf(&mut self) -> String {
        match self.rng.below(5) {
            0 => format!("{}", self.rng.below(1000)),
            1 => format!("{}.5", self.rng.below(100)),
            2 => format!("0x{:x}", self.rng.below(4096)),
            3 => format!("{}", self.rng.below(10)),
            _ => format!("{}e{}", 1 + self.rng.below(9), self.rng.below(4)),
        }
    }

    fn str_expr(&mut self, depth: usize) -> String {
        if depth > 0 && self.rng.chance(40) {
            let lhs = self.str_expr(depth - 1);
            let rhs = if self.rng.chance(50) {
                self.str_expr(depth - 1)
            } else {
                // Numbers coerce in concat.
                self.num_expr(0)
            };
            return format!("({lhs} .. {rhs})");
        }
        match self.rng.below(6) {
            0 => "\"plain\"".to_string(),
            1 => "'single'".to_string(),
            2 => "\"esc\\ttab\\n\"".to_string(),
            3 => "\"d\\101c\"".to_string(), // decimal escape
            4 => "[[long bracket]]".to_string(),
            _ => "\"uni-héllo\"".to_string(),
        }
    }

    fn bool_expr(&mut self, depth: usize) -> String {
        if depth > 0 && self.rng.chance(50) {
            let lhs = self.num_expr(depth - 1);
            let rhs = self.num_expr(depth - 1);
            let op = ["==", "~=", "<", "<=", ">", ">="][self.rng.below(6)];
            return format!("({lhs} {op} {rhs})");
        }
        match self.rng.below(3) {
            0 => "true".to_string(),
            1 => "false".to_string(),
            _ => match self.visible(Kind::Bool) {
                Some(name) => name,
                None => "true".to_string(),
            },
        }
    }

    fn table_expr(&mut self) -> String {
        let len = 1 + self.rng.below(4);
        let mut fields = Vec::new();
        for _ in 0..len {
            fields.push(self.num_expr(1));
        }
        if self.rng.chance(40) {
            let key_value = self.num_expr(0);
            fields.push(format!("named = {key_value}"));
        }
        format!("{{ {} }}", fields.join(", "))
    }

    fn expr_of(&mut self, kind: Kind, depth: usize) -> String {
        match kind {
            Kind::Num => self.num_expr(depth),
            Kind::Str => self.str_expr(depth),
            Kind::Bool => self.bool_expr(depth),
            Kind::Table => self.table_expr(),
        }
    }

    fn random_kind(&mut self) -> Kind {
        match self.rng.below(10) {
            0..=4 => Kind::Num,
            5..=6 => Kind::Str,
            7..=8 => Kind::Bool,
            _ => Kind::Table,
        }
    }

    fn statement(&mut self) {
        let can_nest = self.depth < MAX_DEPTH;
        match self.rng.below(12) {
            0..=2 => self.local_decl(),
            3 => self.assignment(),
            4 if can_nest => self.if_stmt(),
            5 if can_nest => self.numeric_for(),
            6 if can_nest => self.while_stmt(),
            7 if can_nest => self.do_block(),
            8 if can_nest => self.function_decl(),
            9 if can_nest => self.repeat_stmt(),
            10 => self.print_stmt(),
            _ => self.local_decl(),
        }
    }

    fn local_decl(&mut self) {
        let kind = self.random_kind();
        let name = self.fresh_name();
        let value = self.expr_of(kind, 2);
        self.line(&format!("local {name} = {value}"));
        self.declare(name, kind);
    }

    fn assignment(&mut self) {
        if let Some(name) = self.visible(Kind::Num) {
            let value = self.num_expr(2);
            if self.version.has_compound_assignment() && self.rng.chance(30) {
                self.line(&format!("{name} += {value}"));
            } else {
                self.line(&format!("{name} = {value}"));
            }
        } else {
            self.local_decl();
        }
    }

    fn print_stmt(&mut self) {
        let kind = self.random_kind();
        if let Some(name) = self.visible(kind) {
            self.line(&format!("print({name})"));
        } else {
            let value = self.expr_of(kind, 1);
            self.line(&format!("print({value})"));
        }
    }

    fn block<F: FnOnce(&mut Self)>(&mut self, body: F) {
        self.depth += 1;
        self.indent += 1;
        self.scopes.push(Vec::new());
        body(self);
        self.scopes.pop();
        self.indent -= 1;
        self.depth -= 1;
    }

    fn small_body(&mut self) {
        let statement_count = 1 + self.rng.below(3);
        for _ in 0..statement_count {
            self.statement();
        }
    }

    fn if_stmt(&mut self) {
        let condition = self.bool_expr(2);
        self.line(&format!("if {condition} then"));
        self.block(Self::small_body);
        if self.rng.chance(40) {
            let elseif_condition = self.bool_expr(1);
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
        let readonly = self.version.has_const_for_variables();
        let loop_var = if readonly {
            const STEMS: [&str; 6] = ["alpha", "beta", "gamma", "delta", "value", "item"];
            let stem = STEMS[self.rng.below(STEMS.len())];
            self.next_id += 1;
            format!("{stem}{}", self.next_id)
        } else {
            self.fresh_name()
        };
        let stop = 1 + self.rng.below(4);
        self.line(&format!("for {loop_var} = 1, {stop} do"));
        self.depth += 1;
        self.indent += 1;
        self.scopes.push(Vec::new());
        if !readonly {
            self.declare(loop_var, Kind::Num);
        }
        self.small_body();
        self.scopes.pop();
        self.indent -= 1;
        self.depth -= 1;
        self.line("end");
    }

    fn while_stmt(&mut self) {
        // Bounded: counter-driven while.
        let counter = self.fresh_name();
        let limit = 1 + self.rng.below(4);
        self.line(&format!("local {counter} = 0"));
        self.declare(counter.clone(), Kind::Num);
        self.line(&format!("while {counter} < {limit} do"));
        self.block(|generator| {
            generator.line(&format!("{counter} = {counter} + 1"));
            generator.small_body();
        });
        self.line("end");
    }

    fn repeat_stmt(&mut self) {
        let counter = self.fresh_name();
        let limit = 1 + self.rng.below(3);
        self.line(&format!("local {counter} = 0"));
        self.declare(counter.clone(), Kind::Num);
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

    fn function_decl(&mut self) {
        let name = self.fresh_name();
        let param = self.fresh_name();
        self.line(&format!("local function {name}({param})"));
        self.depth += 1;
        self.indent += 1;
        self.scopes.push(Vec::new());
        self.declare(param.clone(), Kind::Num);
        self.small_body();
        let result = self.num_expr(1);
        self.line(&format!("return {param} + {result}"));
        self.scopes.pop();
        self.indent -= 1;
        self.depth -= 1;
        self.line("end");
        // Call it and print the result so the function's body is
        // observable; the declaration itself registers no binding kind we
        // track, so call immediately with a literal.
        let arg = self.rng.below(100);
        self.line(&format!("print({name}({arg}))"));
    }
}

/// Convenience wrapper: one deterministic program per (seed, version).
pub fn generate(seed: u64, version: LuaVersion, statement_budget: usize) -> String {
    Generator::new(seed, version).program(statement_budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic() {
        let first = generate(42, LuaVersion::Lua54, 30);
        let second = generate(42, LuaVersion::Lua54, 30);
        assert_eq!(first, second);
    }

    #[test]
    fn different_seeds_differ() {
        let first = generate(1, LuaVersion::Lua54, 30);
        let second = generate(2, LuaVersion::Lua54, 30);
        assert_ne!(first, second);
    }
}
