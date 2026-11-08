//! Full-grammar generator: parse-only programs that exercise every
//! construct the requested `LuaVersion` accepts.
//!
//! Programs are shaped like real code (module prelude, OOP class table,
//! call- and table-heavy bodies) rather than statement soup.
//!
//! Unlike [`crate::generate`], output isn't runtime-safe: programs are for
//! the parse -> analyze -> transform -> emit pipeline, never for execution,
//! so expressions may index nil, divide strings, or call numbers. Every
//! program must parse with zero errors, and the roundtrip suite checks that
//! across seeds and versions.

use luck_token::LuaVersion;

use crate::rng::Rng;

const MAX_STMT_DEPTH: usize = 4;
const MAX_EXPR_DEPTH: usize = 3;

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

const BUILTIN_TYPES: [&str; 8] =
    ["number", "integer", "string", "boolean", "any", "unknown", "thread", "never"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    Module,
    /// Spliced into a block, where Luau value exports aren't valid.
    Embedded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopEnd {
    End,
    Until,
}

#[derive(Debug)]
struct FullGenerator {
    rng: Rng,
    version: LuaVersion,
    output: String,
    indent: String,
    stmt_depth: usize,
    /// Luau: this program exports values instead of returning a table. A
    /// module can't do both, so each program picks one.
    is_export_module: bool,
    vararg_functions: Vec<bool>,
    names: Vec<String>,
    /// Names the validator treats as read-only: const bindings, exported
    /// functions, and 5.5 `for` control variables. They never appear as
    /// assignment targets.
    readonly_names: Vec<String>,
    callables: Vec<String>,
    /// Type expressions naming a declared alias, generic ones instantiated.
    alias_types: Vec<String>,
    next_id: usize,
    next_label: usize,
}

impl FullGenerator {
    fn new(seed: u64, version: LuaVersion, placement: Placement) -> Self {
        let mut rng = Rng::new(seed);
        let is_export_module =
            placement == Placement::Module && version.has_value_exports() && rng.chance(30);
        Self {
            rng,
            version,
            output: String::new(),
            indent: String::new(),
            stmt_depth: 0,
            is_export_module,
            // The chunk itself is a vararg function in every version.
            vararg_functions: vec![true],
            names: Vec::new(),
            readonly_names: Vec::new(),
            callables: BUILTIN_CALLEES.iter().map(|callee| (*callee).to_owned()).collect(),
            alias_types: Vec::new(),
            next_id: 0,
            next_label: 0,
        }
    }

    fn program(mut self, statement_budget: usize) -> String {
        self.prelude();
        self.class();
        for _ in 0..statement_budget {
            if self.is_export_module && self.rng.chance(15) {
                self.export_declaration();
            } else {
                self.statement();
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

    fn fresh_name(&mut self) -> String {
        let stem = self.rng.pick(&NAME_STEMS);
        self.next_id = self.next_id.saturating_add(1);
        let name = format!("{stem}{}", self.next_id);
        self.names.push(name.clone());
        name
    }

    /// A declared name. The prelude declares two before anything asks.
    fn known_name(&mut self) -> String {
        self.rng.pick(&self.names)
    }

    /// A declared name that isn't read-only. The prelude's never are.
    fn known_writable_name(&mut self) -> String {
        let writable: Vec<&str> = self
            .names
            .iter()
            .filter(|name| !self.readonly_names.contains(name))
            .map(String::as_str)
            .collect();
        self.rng.pick(&writable).to_owned()
    }

    fn known_callable(&mut self) -> String {
        self.rng.pick(&self.callables)
    }

    fn prelude(&mut self) {
        if self.version.is_luau() {
            self.line("--!strict");
        }
        self.line("-- Generated module: exercises the full grammar for one version.");
        self.line("--[[ Long-bracket block comment,");
        self.line("     spanning two lines. ]]");
        self.line("--[==[ leveled block comment ]==]");
        self.line("");
        for _ in 0..2usize {
            let name = self.fresh_name();
            let source = self.rng.pick(&["math.floor", "table.concat", "string.rep", "os.clock"]);
            self.line(&format!("local {name} = {source}"));
        }
        if self.version.is_luau() {
            for _ in 0..2usize {
                self.type_alias_stmt();
            }
        }
        if self.version.has_global() {
            self.line("global registry, dispatch");
            self.line("global <const> *");
        }
        self.line("");
    }

    fn class(&mut self) {
        self.next_id = self.next_id.saturating_add(1);
        let class = format!("Widget{}", self.next_id);
        self.line(&format!("local {class} = {{}}"));
        self.line(&format!("{class}.__index = {class}"));
        self.line("");

        let param = self.fresh_name();
        let annotated =
            if self.version.is_luau() { format!("{param}: number?") } else { param.clone() };
        self.line(&format!("function {class}.new({annotated}, ...)"));
        // Indented by hand, not through `body`, so these statements don't
        // count toward `stmt_depth`.
        self.indent.push('\t');
        self.vararg_functions.push(true);
        self.line(&format!("local self = setmetatable({{}}, {class})"));
        self.line(&format!("self.size = {param} or 0"));
        self.line("self.slots = { ... }");
        self.small_body(2);
        self.line("return self");
        self.vararg_functions.pop();
        self.indent.pop();
        self.line("end");
        self.line("");

        let method = self.rng.pick(&["resize", "update", "render"]);
        let arg = self.fresh_name();
        self.line(&format!("function {class}:{method}({arg})"));
        self.indent.push('\t');
        self.vararg_functions.push(false);
        self.line(&format!("self.size = self.size + {arg} -- trailing comment"));
        self.small_body(2);
        let result = self.expr(2);
        self.line(&format!("return self.size, {result}"));
        self.vararg_functions.pop();
        self.indent.pop();
        self.line("end");
        self.line("");

        self.names.push(class.clone());
        self.callables.push(format!("{class}.new"));
    }

    fn module_return(&mut self) {
        // Luau: an export module may not also return at module scope.
        if self.is_export_module {
            let local = self.fresh_name();
            let value = self.expr(1);
            self.line(&format!("export local {local} = {value}"));
            let function = self.fresh_name();
            self.readonly_names.push(function.clone());
            let param = self.fresh_name();
            self.line(&format!("export function {function}({param}: number)"));
            self.function_body(false);
            return;
        }
        let exported = self.known_name();
        self.vararg_functions.push(false);
        let extra = self.expr(1);
        self.vararg_functions.pop();
        self.line(&format!(
            "return {{ exported = {exported}, build = function() return {extra} end }}"
        ));
    }

    /// Luau: one `export` declaration at module scope. Names come from
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
                self.readonly_names.push(name.clone());
                let value = self.expr(2);
                self.line(&format!("export const {name} = {value}"));
            }
            _ => {
                let name = self.fresh_name();
                self.readonly_names.push(name.clone());
                let param = self.fresh_name();
                if self.rng.chance(30) {
                    self.line("@native");
                }
                self.line(&format!("export function {name}({param}: number)"));
                self.function_body(false);
                self.callables.push(name);
            }
        }
    }

    fn statement(&mut self) {
        let can_nest = self.stmt_depth < MAX_STMT_DEPTH;
        match self.rng.below(24) {
            20 if self.version.is_luau() => self.type_alias_stmt(),
            20 if self.version.has_global() => self.line("global sink"),
            20 if self.version.has_empty_statement() => self.line(";"),
            0..=4 | 20 => self.local_declaration(),
            10 if self.version.has_compound_assignment() => self.compound_assignment(),
            8..=10 => self.assignment(),
            11..=12 if can_nest => self.if_stmt(),
            13 if can_nest => self.numeric_for(),
            14 if can_nest => self.generic_for(),
            15 if can_nest => self.while_stmt(),
            16 if can_nest => self.repeat_stmt(),
            17..=18 if can_nest => self.function_stmt(),
            19 if can_nest => self.do_block(),
            21 => self.multi_local(),
            22 => self.multi_assignment(),
            _ => self.call_stmt(),
        }
    }

    fn local_declaration(&mut self) {
        let name = self.fresh_name();
        let value = self.expr(MAX_EXPR_DEPTH);
        let semicolon = if self.rng.chance(15) { ";" } else { "" };
        if self.version.has_attributes() && self.rng.chance(10) {
            self.readonly_names.push(name.clone());
            if self.version.has_leading_attributes() && self.rng.chance(50) {
                // Lua 5.5
                self.line(&format!("local <const> {name} = {value}{semicolon}"));
            } else {
                // Lua 5.4+
                self.line(&format!("local {name} <const> = {value}{semicolon}"));
            }
        } else if self.version.is_luau() && self.rng.chance(10) {
            // Luau const binding
            self.readonly_names.push(name.clone());
            self.line(&format!("const {name} = {value}{semicolon}"));
        } else if self.version.is_luau() && self.rng.chance(35) {
            let annotation = self.type_expr(2);
            self.line(&format!("local {name}: {annotation} = {value}{semicolon}"));
        } else {
            self.line(&format!("local {name} = {value}{semicolon}"));
        }
    }

    fn multi_local(&mut self) {
        let first = self.fresh_name();
        let second = self.fresh_name();
        let callee = self.known_callable();
        let arg = self.expr(1);
        self.line(&format!("local {first}, {second} = {callee}({arg})"));
    }

    fn assignment(&mut self) {
        let target = self.assign_target();
        let value = self.expr(MAX_EXPR_DEPTH);
        self.line(&format!("{target} = {value}"));
    }

    fn multi_assignment(&mut self) {
        let first = self.assign_target();
        let second = self.assign_target();
        let value_a = self.expr(1);
        let value_b = self.expr(1);
        self.line(&format!("{first}, {second} = {value_b}, {value_a}"));
    }

    fn assign_target(&mut self) -> String {
        let base = self.known_writable_name();
        match self.rng.below(4) {
            0 => base,
            1 => format!("{base}.field"),
            2 => {
                // Spaces keep a long-bracket string key from forming `[[`.
                let key = self.expr(1);
                format!("{base}[ {key} ]")
            }
            _ => format!("{base}.nested.slot"),
        }
    }

    fn compound_assignment(&mut self) {
        // Luau
        let target = self.assign_target();
        let op = self.rng.pick(&["+=", "-=", "*=", "/=", "//=", "%=", "^=", "..="]);
        let value = self.expr(2);
        self.line(&format!("{target} {op} {value}"));
    }

    fn call_stmt(&mut self) {
        let callee = self.known_callable();
        match self.rng.below(6) {
            0 => {
                let receiver = self.known_name();
                let method = self.rng.pick(&["update", "resize", "render", "insert"]);
                let arg = self.expr(2);
                self.line(&format!("{receiver}:{method}({arg})"));
            }
            1 => self.line(&format!("{callee} \"literal argument\"")),
            2 => {
                let field = self.expr(1);
                self.line(&format!("{callee} {{ tag = \"item\", {field} }}"));
            }
            3 => {
                let first = self.expr(2);
                let second = self.expr(1);
                self.line(&format!("{callee}({first}, {second})"));
            }
            4 => self.line(&format!("{callee} [[long-bracket argument]]")),
            _ => {
                let arg = self.expr(2);
                self.line(&format!("{callee}({arg})"));
            }
        }
    }

    fn if_stmt(&mut self) {
        let condition = self.condition();
        self.line(&format!("if {condition} then"));
        self.body(|generator| generator.small_body(2));
        if self.rng.chance(40) {
            let elseif_condition = self.condition();
            self.line(&format!("elseif {elseif_condition} then"));
            self.body(|generator| generator.small_body(1));
        }
        if self.rng.chance(45) {
            self.line("else");
            self.body(|generator| generator.small_body(1));
        }
        self.line("end");
    }

    fn numeric_for(&mut self) {
        let loop_variable = self.fresh_name();
        // 5.5 makes for control variables read-only.
        if self.version.has_const_for_variables() {
            self.readonly_names.push(loop_variable.clone());
        }
        let stop = self.expr(1);
        let header = if self.rng.chance(40) {
            format!("for {loop_variable} = 1, {stop}, 2 do")
        } else {
            format!("for {loop_variable} = 1, {stop} do")
        };
        self.line(&header);
        self.loop_body(LoopEnd::End);
        self.line("end");
    }

    fn generic_for(&mut self) {
        let key = self.fresh_name();
        let value = self.fresh_name();
        if self.version.has_const_for_variables() {
            self.readonly_names.push(key.clone());
            self.readonly_names.push(value.clone());
        }
        let subject = self.known_name();
        let iterator = match self.rng.below(3) {
            0 => format!("ipairs({subject})"),
            1 => format!("pairs({subject})"),
            _ => format!("next, {subject}, nil"),
        };
        self.line(&format!("for {key}, {value} in {iterator} do"));
        self.loop_body(LoopEnd::End);
        self.line("end");
    }

    fn while_stmt(&mut self) {
        let condition = self.condition();
        self.line(&format!("while {condition} do"));
        self.loop_body(LoopEnd::End);
        self.line("end");
    }

    fn repeat_stmt(&mut self) {
        // No goto-continue idiom here: a repeat block's trailing label still
        // precedes `until`, which can see the block's locals, so Lua rejects
        // a goto that jumps over any of them.
        self.line("repeat");
        self.loop_body(LoopEnd::Until);
        let condition = self.condition();
        self.line(&format!("until {condition}"));
    }

    fn do_block(&mut self) {
        self.line("do");
        self.body(|generator| generator.small_body(2));
        self.line("end");
    }

    /// Loop bodies exercise the exit statements: `break` (as the last
    /// statement of an `if` block, valid in every version including the
    /// 5.1/Luau last-statement restriction), Luau `continue`, and the
    /// 5.2+ goto-continue idiom with the label in end-of-block position.
    fn loop_body(&mut self, loop_end: LoopEnd) {
        let has_goto_continue =
            loop_end == LoopEnd::End && self.version.has_goto() && self.rng.chance(30);
        let continue_label = has_goto_continue.then(|| {
            self.next_label = self.next_label.saturating_add(1);
            format!("continue_{}", self.next_label)
        });
        self.body(|generator| {
            if let Some(label) = &continue_label {
                let condition = generator.condition();
                generator.line(&format!("if {condition} then goto {label} end"));
            }
            generator.small_body(2);
            if generator.rng.chance(25) {
                let condition = generator.condition();
                generator.line(&format!("if {condition} then break end"));
            }
            match &continue_label {
                Some(label) => generator.line(&format!("::{label}::")),
                // Luau: last statement of the block only.
                None if generator.version.has_continue() && generator.rng.chance(20) => {
                    generator.line("continue");
                }
                None => {}
            }
        });
    }

    fn function_stmt(&mut self) {
        match self.rng.below(4) {
            1 => self.dotted_function(),
            2 if self.version.has_global() => {
                let name = self.fresh_name();
                self.line(&format!("global function {name}()"));
                self.function_body(false);
                self.callables.push(name);
            }
            _ => self.local_function(),
        }
    }

    fn local_function(&mut self) {
        let name = self.fresh_name();
        let param = self.fresh_name();
        let is_vararg = self.rng.chance(40);
        let generics = if self.version.is_luau() && self.rng.chance(25) { "<T>" } else { "" };
        let vararg_param = if is_vararg {
            if self.version.has_named_varargs() && self.rng.chance(50) {
                ", ...rest"
            } else {
                ", ..."
            }
        } else {
            ""
        };
        let signature = if self.version.is_luau() {
            let annotation = self.type_expr(1);
            format!("local function {name}{generics}({param}: {annotation}{vararg_param}): number")
        } else {
            format!("local function {name}({param}{vararg_param})")
        };
        self.line(&signature);
        self.function_body(is_vararg);
        self.callables.push(name);
    }

    fn dotted_function(&mut self) {
        let base = self.known_name();
        let leaf = self.fresh_name();
        let is_method = self.rng.chance(50);
        let separator = if is_method { ":" } else { "." };
        let param = self.fresh_name();
        self.line(&format!("function {base}{separator}{leaf}({param})"));
        self.function_body(false);
        if !is_method {
            self.callables.push(format!("{base}.{leaf}"));
        }
    }

    fn function_body(&mut self, is_vararg: bool) {
        self.body(|generator| {
            generator.vararg_functions.push(is_vararg);
            generator.small_body(2);
            let result = generator.expr(2);
            if generator.rng.chance(30) {
                let second = generator.expr(1);
                generator.line(&format!("return {result}, {second}"));
            } else if generator.rng.chance(15) {
                generator.line(&format!("return {result};"));
            } else {
                generator.line(&format!("return {result}"));
            }
            generator.vararg_functions.pop();
        });
        self.line("end");
    }

    fn body<F>(&mut self, fill: F)
    where
        F: FnOnce(&mut Self),
    {
        self.indent.push('\t');
        self.stmt_depth = self.stmt_depth.saturating_add(1);
        fill(self);
        self.stmt_depth = self.stmt_depth.saturating_sub(1);
        self.indent.pop();
    }

    fn small_body(&mut self, budget: usize) {
        let count = self.rng.one_through(budget);
        for _ in 0..count {
            self.statement();
        }
    }

    fn condition(&mut self) -> String {
        let lhs = self.expr(1);
        match self.rng.below(4) {
            0 => {
                let rhs = self.expr(1);
                let op = self.rng.pick(&["==", "~=", "<", "<=", ">", ">="]);
                format!("{lhs} {op} {rhs}")
            }
            1 => format!("not ({lhs})"),
            2 => {
                let rhs = self.expr(1);
                format!("({lhs}) and ({rhs})")
            }
            _ => lhs,
        }
    }

    fn expr(&mut self, depth: usize) -> String {
        let Some(inner_depth) = depth.checked_sub(1) else {
            return self.leaf_expr();
        };
        match self.rng.below(14) {
            0..=2 => self.leaf_expr(),
            3..=5 => {
                let lhs = self.expr(inner_depth);
                let rhs = self.expr(inner_depth);
                let op = self.binary_op();
                format!("({lhs} {op} {rhs})")
            }
            6 => {
                let operand = self.expr(inner_depth);
                let op = self.unary_op();
                format!("({op}{operand})")
            }
            7 => self.call_expr(inner_depth),
            8 => self.table_expr(inner_depth),
            9 => self.index_expr(),
            10 => self.function_expr(),
            11 if self.version.is_luau() => self.luau_expr(inner_depth),
            12 if self.is_vararg_allowed() => "...".to_owned(),
            _ => {
                let inner = self.expr(inner_depth);
                format!("({inner})")
            }
        }
    }

    fn is_vararg_allowed(&self) -> bool {
        self.vararg_functions.last() == Some(&true)
    }

    fn binary_op(&mut self) -> &'static str {
        let mut ops =
            vec!["+", "-", "*", "/", "%", "^", "..", "==", "~=", "<", "<=", ">", ">=", "and", "or"];
        if self.version.has_floor_div() {
            ops.push("//");
        }
        if self.version.has_bitwise_ops() {
            ops.extend(["&", "|", "~", "<<", ">>"]);
        }
        self.rng.pick(&ops)
    }

    fn unary_op(&mut self) -> &'static str {
        let mut ops = vec!["-", "not ", "#"];
        if self.version.has_bitwise_ops() {
            ops.push("~");
        }
        self.rng.pick(&ops)
    }

    fn leaf_expr(&mut self) -> String {
        match self.rng.below(10) {
            0..=2 => self.known_name(),
            3..=4 => self.number_literal(),
            5..=6 => self.string_literal(),
            7 => "true".to_owned(),
            8 => "nil".to_owned(),
            _ => "false".to_owned(),
        }
    }

    fn number_literal(&mut self) -> String {
        match self.rng.below(10) {
            0 => format!("{}", self.rng.below(10_000)),
            1 => format!("{}.{}", self.rng.below(100), self.rng.below(100)),
            2 => format!("0x{:X}", self.rng.below(0xFFFF)),
            3 => format!("{}e{}", self.rng.one_through(9), self.rng.below(6)),
            4 => format!("{}.5e-{}", self.rng.below(50), self.rng.one_through(3)),
            5 if self.version.has_hex_floats() => {
                format!("0x{:x}.8p{}", self.rng.one_through(15), self.rng.below(4))
            }
            6 if self.version.has_binary_literals() => {
                format!("0b{:b}", self.rng.below(256))
            }
            7 if self.version.has_underscore_separators() => "1_000_000".to_owned(),
            8 if self.version.has_luau_integer_literals() => "1_000_000i".to_owned(),
            9 if self.version.has_luau_integer_literals() => match self.rng.below(3) {
                0 => format!("{}i", self.rng.below(10_000)),
                1 => format!("0x{:X}i", self.rng.below(0xFFFF)),
                _ => format!("0b{:b}i", self.rng.below(256)),
            },
            _ => format!("{}", self.rng.below(100)),
        }
    }

    fn string_literal(&mut self) -> String {
        match self.rng.below(9) {
            0 => "\"plain double\"".to_owned(),
            1 => "'plain single'".to_owned(),
            2 => "\"escaped\\ttab\\nline\\\\slash\"".to_owned(),
            3 => "\"dec\\101scape\"".to_owned(),
            4 if self.version.has_hex_escape() => "\"hex\\x41escape\"".to_owned(),
            5 if self.version.has_whitespace_escape() => "\"skip\\z   joined\"".to_owned(),
            6 if self.version.has_unicode_escape() => "\"uni\\u{48}escape\"".to_owned(),
            7 => "[[long bracket string]]".to_owned(),
            8 => "[==[leveled ]] bracket]==]".to_owned(),
            _ => "\"fallback\"".to_owned(),
        }
    }

    fn call_expr(&mut self, arg_depth: usize) -> String {
        let callee = self.known_callable();
        match self.rng.below(5) {
            0 => {
                let receiver = self.known_name();
                let arg = self.expr(arg_depth);
                format!("{receiver}:clone({arg})")
            }
            1 => format!("{callee} \"inline\""),
            2 => {
                let element = self.expr(arg_depth);
                format!("{callee} {{ {element} }}")
            }
            3 => {
                let first = self.expr(arg_depth);
                let second = self.expr(arg_depth);
                format!("{callee}({first}, {second})")
            }
            _ => {
                let arg = self.expr(arg_depth);
                format!("{callee}({arg})")
            }
        }
    }

    fn index_expr(&mut self) -> String {
        let base = self.known_name();
        match self.rng.below(4) {
            0 => format!("{base}.field"),
            1 => format!("{base}.nested.deep.chain"),
            2 => {
                // Spaces keep a long-bracket string key from forming `[[`.
                let key = self.expr(1);
                format!("{base}[ {key} ]")
            }
            _ => format!("{base}[\"quoted key\"]"),
        }
    }

    fn table_expr(&mut self, field_depth: usize) -> String {
        let field_count = self.rng.below(5);
        if field_count == 0 {
            return "{}".to_owned();
        }
        let mut fields = Vec::new();
        for _ in 0..field_count {
            let field = match self.rng.below(4) {
                0 => self.expr(field_depth),
                1 => {
                    let value = self.expr(field_depth);
                    format!("label = {value}")
                }
                2 => {
                    // Spaces keep a long-bracket string key from forming `[[`.
                    let key = self.expr(0);
                    let value = self.expr(field_depth);
                    format!("[ {key} ] = {value}")
                }
                _ => {
                    let value = self.expr(field_depth);
                    format!("[\"bracket key\"] = {value}")
                }
            };
            fields.push(field);
        }
        let separator = if self.rng.chance(20) { "; " } else { ", " };
        let trailing = if self.rng.chance(25) { separator.trim_end() } else { "" };
        format!("{{ {}{trailing} }}", fields.join(separator))
    }

    fn function_expr(&mut self) -> String {
        let param = self.fresh_name();
        let is_vararg = self.rng.chance(30);
        // The body expression must respect the nested function's vararg
        // status, not the enclosing one's, or `...` leaks into a
        // non-vararg function.
        self.vararg_functions.push(is_vararg);
        let body_expr = self.expr(1);
        self.vararg_functions.pop();
        if is_vararg {
            format!("function(...) return select(\"#\", ...), {body_expr} end")
        } else {
            format!("function({param}) return {param} + {body_expr} end")
        }
    }

    fn luau_expr(&mut self, inner_depth: usize) -> String {
        match self.rng.below(6) {
            0 => {
                let operand = self.expr(inner_depth);
                let annotation = self.type_expr(1);
                format!("(({operand}) :: {annotation})")
            }
            1 => {
                let condition = self.condition();
                let then_value = self.expr(inner_depth);
                let else_value = self.expr(inner_depth);
                format!("(if {condition} then {then_value} else {else_value})")
            }
            2 => {
                // Leaf expressions only: a `{`-starting table would form
                // `{{`, which Luau rejects inside interpolated strings.
                let first = self.expr(0);
                let second = self.expr(0);
                format!("`count {{{first}}}, next {{{second}}}`")
            }
            4 if self.version.has_explicit_type_instantiation() => {
                let callee = self.known_callable();
                let type_arg = self.leaf_type();
                let arg = self.expr(inner_depth);
                format!("{callee}<<{type_arg}>>({arg})")
            }
            5 if self.version.has_explicit_type_instantiation() => {
                let receiver = self.known_name();
                let type_arg = self.leaf_type();
                format!("{receiver}:clone<<{type_arg}>>()")
            }
            _ => {
                let condition = self.condition();
                let then_value = self.expr(0);
                let middle = self.expr(0);
                let else_value = self.expr(0);
                format!(
                    "(if {condition} then {then_value} elseif {condition} then {middle} else {else_value})"
                )
            }
        }
    }

    fn type_alias_stmt(&mut self) {
        self.next_id = self.next_id.saturating_add(1);
        let name = format!("Alias{}", self.next_id);
        let export_prefix =
            if self.indent.is_empty() && self.rng.chance(40) { "export " } else { "" };
        let is_generic = self.rng.chance(30);
        let body = self.type_expr(2);
        if is_generic {
            self.line(&format!("{export_prefix}type {name}<T> = {{ value: T, tail: {body} }}"));
            self.alias_types.push(format!("{name}<string>"));
        } else {
            self.line(&format!("{export_prefix}type {name} = {body}"));
            self.alias_types.push(name);
        }
    }

    fn type_expr(&mut self, depth: usize) -> String {
        let Some(inner_depth) = depth.checked_sub(1) else {
            return self.leaf_type();
        };
        match self.rng.below(11) {
            0..=2 => self.leaf_type(),
            3 => {
                let inner = self.rng.pick(&BUILTIN_TYPES);
                format!("{inner}?")
            }
            4 => {
                let lhs = self.type_expr(inner_depth);
                let rhs = self.type_expr(inner_depth);
                format!("({lhs}) | ({rhs})")
            }
            5 => {
                let lhs = self.leaf_type();
                let rhs = self.leaf_type();
                format!("({lhs}) & ({rhs})")
            }
            6 => {
                let element = self.type_expr(inner_depth);
                format!("{{ {element} }}")
            }
            7 => {
                let field = self.type_expr(inner_depth);
                let indexed = self.leaf_type();
                format!("{{ label: {field}, [string]: {indexed} }}")
            }
            8 => {
                let param = self.leaf_type();
                let return_type = self.type_expr(inner_depth);
                match self.rng.below(3) {
                    0 => format!("({param}) -> {return_type}"),
                    1 => format!("({param}, ...{param}) -> ({return_type})"),
                    _ => "() -> ()".to_owned(),
                }
            }
            _ => {
                let subject = self.known_name();
                format!("typeof({subject})")
            }
        }
    }

    fn leaf_type(&mut self) -> String {
        if !self.alias_types.is_empty() && self.rng.chance(25) {
            return self.rng.pick(&self.alias_types);
        }
        match self.rng.below(9) {
            0..=5 => self.rng.pick(&BUILTIN_TYPES).to_owned(),
            6 => "\"singleton\"".to_owned(),
            7 => "true".to_owned(),
            _ => "nil".to_owned(),
        }
    }
}

/// One deterministic full-grammar program per (seed, version, budget), with
/// `statement_budget` top-level statements between the class and the module
/// return.
#[must_use]
pub fn generate_full(seed: u64, version: LuaVersion, statement_budget: usize) -> String {
    FullGenerator::new(seed, version, Placement::Module).program(statement_budget)
}

/// Like [`generate_full`], but safe to splice into a nested block
/// (`do ... end` wrappers): Luau value exports are only valid at module
/// scope, so the export-module mode is disabled.
#[must_use]
pub fn generate_full_embeddable(seed: u64, version: LuaVersion, statement_budget: usize) -> String {
    FullGenerator::new(seed, version, Placement::Embedded).program(statement_budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_generates_the_same_program() {
        let first = generate_full(7, LuaVersion::Luau, 30);
        let second = generate_full(7, LuaVersion::Luau, 30);
        assert_eq!(first, second);
    }

    #[test]
    fn lua51_programs_contain_no_later_syntax() {
        for seed in 0..8 {
            let lua51 = generate_full(seed, LuaVersion::Lua51, 60);
            assert!(!lua51.contains("goto"), "5.1 must not emit goto");
            assert!(!lua51.contains("::"), "5.1 must not emit labels");
            assert!(!lua51.contains("<const>"), "5.1 must not emit attributes");
            assert!(!lua51.contains("continue"), "5.1 must not emit continue");
            assert!(!lua51.contains("0b"), "5.1 must not emit binary literals");
            assert!(!lua51.contains("export local"), "5.1 must not emit exports");
            assert!(!lua51.contains("export const"), "5.1 must not emit exports");
            assert!(!lua51.contains("export function"), "5.1 must not emit exports");
            assert!(!lua51.contains("<<"), "5.1 must not emit instantiation");
            assert!(!lua51.contains("1_000_000i"), "5.1 must not emit integer literals");
        }
    }

    #[test]
    fn luau_programs_reach_merged_rfc_syntax() {
        let mut all_programs = String::new();
        for seed in 0..16 {
            all_programs.push_str(&generate_full(seed, LuaVersion::Luau, 60));
        }
        for needle in ["export local", "export const", "export function", "1_000_000i", "<<"] {
            assert!(all_programs.contains(needle), "expected generated {needle}");
        }
    }

    #[test]
    fn embeddable_programs_export_no_values() {
        for seed in 0..16 {
            let program = generate_full_embeddable(seed, LuaVersion::Luau, 60);
            for needle in ["export local", "export const", "export function"] {
                assert!(!program.contains(needle), "seed {seed} emitted {needle}");
            }
        }
    }
}
