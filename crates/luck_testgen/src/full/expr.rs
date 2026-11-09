use super::{FullGenerator, Varargs};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the generator is one type with its grammar split by file"
)]
impl FullGenerator {
    pub(super) fn condition(&mut self) -> String {
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

    pub(super) fn expr(&mut self, depth: usize) -> String {
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
        self.function_varargs.last() == Some(&Varargs::Allowed)
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
            5..=6 => self.string_literal().to_owned(),
            7 => "true".to_owned(),
            8 => "nil".to_owned(),
            _ => "false".to_owned(),
        }
    }

    fn number_literal(&mut self) -> String {
        match self.rng.below(10) {
            0 => self.rng.below(10_000).to_string(),
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
            _ => self.rng.below(100).to_string(),
        }
    }

    fn string_literal(&mut self) -> &'static str {
        match self.rng.below(9) {
            0 => "\"plain double\"",
            1 => "'plain single'",
            2 => "\"escaped\\ttab\\nline\\\\slash\"",
            3 => "\"dec\\101scape\"",
            4 if self.version.has_hex_escape() => "\"hex\\x41escape\"",
            5 if self.version.has_whitespace_escape() => "\"skip\\z   joined\"",
            6 if self.version.has_unicode_escape() => "\"uni\\u{48}escape\"",
            7 => "[[long bracket string]]",
            8 => "[==[leveled ]] bracket]==]",
            _ => "\"fallback\"",
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
        let varargs = if self.rng.chance(30) { Varargs::Allowed } else { Varargs::Forbidden };
        // The body expression must respect the nested function's vararg
        // status, not the enclosing one's, or `...` leaks into a
        // non-vararg function.
        let body_expr = self.with_varargs(varargs, |generator| generator.expr(1));
        match varargs {
            Varargs::Allowed => format!("function(...) return select(\"#\", ...), {body_expr} end"),
            Varargs::Forbidden => format!("function({param}) return {param} + {body_expr} end"),
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
}
