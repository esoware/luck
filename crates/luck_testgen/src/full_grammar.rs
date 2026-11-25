mod expr;
mod stmt;
mod types;

use crate::rng::Rng;

const NAME_STEMS: [&str; 16] = [
    "config", "state", "result", "index", "count", "buffer", "handler", "options", "payload",
    "cache", "total", "node", "queue", "widget", "service", "entry",
];

const BUILTIN_CALLEES: [&str; 12] = [
    "print",
    "tostring",
    "tonumber",
    "pcall",
    "select",
    "type",
    "ipairs",
    "setmetatable",
    "string.format",
    "table.insert",
    "math.max",
    "math.floor",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Varargs {
    Allowed,
    Forbidden,
}

#[derive(Debug)]
struct FullGenerator {
    rng: Rng,
    output: String,
    indent: String,
    stmt_depth: usize,
    /// This program exports values instead of returning a table. A module
    /// can't do both, so each program picks one.
    is_export_module: bool,
    function_varargs: Vec<Varargs>,
    names: Vec<String>,
    /// Names the validator treats as read-only: const bindings and exported
    /// functions. They never appear as assignment targets.
    read_only_names: Vec<String>,
    callables: Vec<String>,
    /// Type expressions naming a declared alias, generic ones instantiated.
    alias_types: Vec<String>,
    next_id: usize,
}

impl FullGenerator {
    fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let is_export_module = rng.chance(30);
        Self {
            rng,
            output: String::new(),
            indent: String::new(),
            stmt_depth: 0,
            is_export_module,
            // The chunk itself is a vararg function.
            function_varargs: vec![Varargs::Allowed],
            names: Vec::new(),
            read_only_names: Vec::new(),
            callables: BUILTIN_CALLEES.iter().map(|callee| (*callee).to_owned()).collect(),
            alias_types: Vec::new(),
            next_id: 0,
        }
    }

    fn program(mut self, statement_budget: usize) -> String {
        self.prelude();
        self.class();
        for _ in 0..statement_budget {
            if self.is_export_module && self.rng.chance(15) {
                self.export_declaration();
            } else {
                self.stmt();
            }
        }
        self.module_return();
        self.output
    }

    fn line(&mut self, text: &str) {
        self.output.push_str(&self.indent);
        self.output.push_str(text);
        self.output.push('\n');
    }

    const fn fresh_id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    fn fresh_name(&mut self) -> String {
        let stem = self.rng.pick(&NAME_STEMS);
        let name = format!("{stem}{}", self.fresh_id());
        self.names.push(name.clone());
        name
    }

    /// A declared name. The prelude declares two before anything asks.
    fn known_name(&mut self) -> String {
        self.rng.pick(&self.names)
    }

    /// A declared name that isn't read-only. The prelude's never are.
    fn known_assignable_name(&mut self) -> String {
        let assignable: Vec<&str> = self
            .names
            .iter()
            .filter(|name| !self.read_only_names.contains(name))
            .map(String::as_str)
            .collect();
        self.rng.pick(&assignable).to_owned()
    }

    fn known_callable(&mut self) -> String {
        self.rng.pick(&self.callables)
    }

    fn prelude(&mut self) {
        self.line("--!strict");
        self.line("-- Generated module: exercises the full grammar.");
        self.line("--[[ Long-bracket block comment,");
        self.line("     spanning two lines. ]]");
        self.line("--[==[ leveled block comment ]==]");
        self.line("");
        for _ in 0..2 {
            let name = self.fresh_name();
            let source = self.rng.pick(&["math.floor", "table.concat", "string.rep", "os.clock"]);
            self.line(&format!("local {name} = {source}"));
        }
        for _ in 0..2 {
            self.type_alias_stmt();
        }
        self.line("");
    }

    fn class(&mut self) {
        let class = format!("Widget{}", self.fresh_id());
        self.line(&format!("local {class} = {{}}"));
        self.line(&format!("{class}.__index = {class}"));
        self.line("");

        let param = self.fresh_name();
        self.line(&format!("function {class}.new({param}: number?, ...)"));
        self.class_function_body(Varargs::Allowed, |generator| {
            generator.line(&format!("local self = setmetatable({{}}, {class})"));
            generator.line(&format!("self.size = {param} or 0"));
            generator.line("self.slots = { ... }");
            generator.small_body(2);
            generator.line("return self");
        });

        let method = self.rng.pick(&["resize", "update", "render"]);
        let arg = self.fresh_name();
        self.line(&format!("function {class}:{method}({arg})"));
        self.class_function_body(Varargs::Forbidden, |generator| {
            generator.line(&format!("self.size = self.size + {arg} -- trailing comment"));
            generator.small_body(2);
            let result = generator.expr(2);
            generator.line(&format!("return self.size, {result}"));
        });

        self.names.push(class.clone());
        self.callables.push(format!("{class}.new"));
    }

    /// Indented by hand, not through `block`, so the body's statements don't
    /// count toward `stmt_depth`.
    fn class_function_body<F>(&mut self, varargs: Varargs, fill: F)
    where
        F: FnOnce(&mut Self),
    {
        self.indent.push('\t');
        self.with_varargs(varargs, fill);
        self.indent.pop();
        self.line("end");
        self.line("");
    }

    fn with_varargs<R, F>(&mut self, varargs: Varargs, fill: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.function_varargs.push(varargs);
        let filled = fill(self);
        self.function_varargs.pop();
        filled
    }

    fn module_return(&mut self) {
        // An export module may not also return at module scope.
        if self.is_export_module {
            let local = self.fresh_name();
            let value = self.expr(1);
            self.line(&format!("export local {local} = {value}"));
            let function = self.fresh_name();
            self.read_only_names.push(function.clone());
            let param = self.fresh_name();
            self.line(&format!("export function {function}({param}: number)"));
            self.function_body(Varargs::Forbidden);
            return;
        }
        let exported = self.known_name();
        let extra = self.with_varargs(Varargs::Forbidden, |generator| generator.expr(1));
        self.line(&format!(
            "return {{ exported = {exported}, build = function() return {extra} end }}"
        ));
    }

    /// One `export` declaration at module scope. Names come from
    /// `fresh_name`, which never repeats, so duplicate exports can't happen.
    fn export_declaration(&mut self) {
        match self.rng.below(3) {
            0 => {
                let name = self.fresh_name();
                let annotation = self.type_expr(1);
                let value = self.expr(2);
                self.line(&format!("export local {name}: {annotation} = {value}"));
            }
            1 => {
                let name = self.fresh_name();
                self.read_only_names.push(name.clone());
                let value = self.expr(2);
                self.line(&format!("export const {name} = {value}"));
            }
            _ => {
                let name = self.fresh_name();
                self.read_only_names.push(name.clone());
                let param = self.fresh_name();
                if self.rng.chance(30) {
                    self.line("@native");
                }
                self.line(&format!("export function {name}({param}: number)"));
                self.function_body(Varargs::Forbidden);
                self.callables.push(name);
            }
        }
    }
}

/// A program that reaches the whole grammar, with `statement_budget` random
/// statements at module scope.
///
/// The same arguments always give the same program. It's shaped like real
/// code, a module prelude, a class table, and call- and table-heavy bodies,
/// rather than statement soup. It parses with no errors, but it's for the
/// stages, never for running, so it may index `nil`, divide strings, or call
/// numbers.
pub fn generate_full_grammar(seed: u64, statement_budget: usize) -> String {
    FullGenerator::new(seed).program(statement_budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_generates_the_same_program() {
        let first = generate_full_grammar(7, 30);
        let second = generate_full_grammar(7, 30);
        assert_eq!(first, second);
    }

    #[test]
    fn programs_reach_merged_rfc_syntax() {
        let mut all_programs = String::new();
        for seed in 0..16 {
            all_programs.push_str(&generate_full_grammar(seed, 60));
        }
        for needle in ["export local", "export const", "export function", "1_000_000i", "<<"] {
            assert!(all_programs.contains(needle), "expected generated {needle}");
        }
    }
}
