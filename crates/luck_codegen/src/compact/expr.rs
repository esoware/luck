use luck_ast::{
    Expression, Field, FunctionArgs, Suffix, SuffixedExpression, TableConstructor, Var,
};

use super::Printer;
use crate::separator::{INTERPOLATION_BRACE, OTHER, merges_after};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the printer is one type with its node families split by file"
)]
impl Printer {
    /// Names, numbers, and strings, half of all expressions, print inline
    /// where they're reached, which saves them a call to a function that
    /// saves every callee-saved register.
    #[expect(clippy::inline_always, reason = "an outlined copy would be the call it saves")]
    #[inline(always)]
    pub(super) fn print_expr(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::Name(name) => self.name(name.text.as_str()),
            Expression::Number(literal) | Expression::Integer(literal) => {
                self.number(literal.text);
            }
            Expression::StringLiteral(literal) => self.string(literal.text),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => self.print_other_expr(expr),
        }
    }

    fn print_other_expr(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::Name(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_) => self.print_expr(expr),
            Expression::Nil(_) => self.piece("nil"),
            Expression::False(_) => self.piece("false"),
            Expression::True(_) => self.piece("true"),
            // `.....` reads as `...` `..`, so nothing merges with a vararg.
            Expression::VarArg(_) => self.emit("...", merges_after(b'.'), OTHER),
            Expression::FunctionDef(function) => {
                self.print_function_attributes(&function.attributes);
                self.piece("function");
                self.print_function_body(&function.body);
            }
            Expression::Suffixed(chain) => self.print_suffixed(chain),
            Expression::Parenthesized(parenthesized) => {
                self.piece("(");
                self.print_expr(&parenthesized.expr);
                self.piece(")");
            }
            Expression::TableConstructor(table) => self.print_table(table),
            Expression::BinaryOp(binary_op) => {
                self.print_expr(&binary_op.first);
                for link in &binary_op.links {
                    self.piece(link.op.static_text());
                    self.print_expr(&link.operand);
                }
            }
            Expression::UnaryOp(unary_op) => {
                self.piece(unary_op.op.static_text());
                self.print_expr(&unary_op.operand);
            }
            // Luau
            Expression::IfExpression(if_expr) => {
                self.piece("if");
                self.print_expr(&if_expr.condition);
                self.piece("then");
                self.print_expr(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    self.piece("elseif");
                    self.print_expr(&clause.condition);
                    self.piece("then");
                    self.print_expr(&clause.expr);
                }
                self.piece("else");
                self.print_expr(&if_expr.else_expr);
            }
            // Luau
            Expression::InterpolatedString(interpolated) => {
                self.output.print_ascii_byte(b'`');
                for segment in &interpolated.segments {
                    self.output.print_str(segment.text.text);
                    self.output.print_ascii_byte(b'{');
                    self.last = INTERPOLATION_BRACE;
                    self.print_expr(&segment.expr);
                    self.output.print_ascii_byte(b'}');
                }
                self.output.print_str(interpolated.tail.text);
                self.output.print_ascii_byte(b'`');
                self.last = OTHER;
            }
            // Luau
            Expression::TypeCast(cast) => {
                self.print_expr(&cast.expr);
                self.piece("::");
                self.print_type(&cast.ty);
            }
            Expression::Error(_) => {}
        }
    }

    pub(super) fn print_var(&mut self, var: &Var<'_>) {
        match var {
            Var::Name(name) => self.name(name.text.as_str()),
            Var::Suffixed(chain) => self.print_suffixed(chain),
        }
    }

    #[expect(
        clippy::inline_always,
        reason = "measured: inlined into each caller, it prints up to 3% faster"
    )]
    #[inline(always)]
    pub(super) fn print_suffixed(&mut self, chain: &SuffixedExpression<'_>) {
        self.print_expr(&chain.prefix);
        for suffix in &chain.suffixes {
            match suffix {
                Suffix::Field(name) => {
                    self.piece(".");
                    self.name(name.text.as_str());
                }
                Suffix::Index { index, .. } => {
                    self.piece("[");
                    self.print_expr(index);
                    self.piece("]");
                }
                Suffix::Call(args) => self.print_args(args),
                Suffix::MethodCall(call) => {
                    self.piece(":");
                    self.name(call.name.text.as_str());
                    if let Some(type_args) = &call.type_args {
                        self.print_type_instantiation(type_args);
                    }
                    self.print_args(&call.args);
                }
                // Luau
                Suffix::TypeInstantiation { type_args, .. } => {
                    self.print_type_instantiation(type_args);
                }
            }
        }
    }

    fn print_args(&mut self, function_args: &FunctionArgs<'_>) {
        match function_args {
            FunctionArgs::Parenthesized { args, .. } => {
                self.piece("(");
                self.list(args, ",", Self::print_expr);
                self.piece(")");
            }
            FunctionArgs::TableConstructor(table) => self.print_table(table),
            FunctionArgs::StringLiteral(literal) => self.string(literal.text),
            FunctionArgs::Error(_) => {}
        }
    }

    /// A trailing separator changes nothing, so it's dropped.
    fn print_table(&mut self, table: &TableConstructor<'_>) {
        self.piece("{");
        self.list(&table.fields, ",", Self::print_field);
        self.piece("}");
    }

    fn print_field(&mut self, field: &Field<'_>) {
        match field {
            Field::Bracketed { key, value, .. } => {
                self.piece("[");
                self.print_expr(key);
                self.piece("]=");
                self.print_expr(value);
            }
            Field::Named { name, value } => {
                self.name(name.text.as_str());
                self.piece("=");
                self.print_expr(value);
            }
            Field::Positional(value) => self.print_expr(value),
        }
    }
}
