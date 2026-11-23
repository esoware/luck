use luck_ast::{
    BinaryOp, Expression, Field, FunctionArgs, IfExpression, InterpolatedString, Slot, Suffix,
    SuffixedExpression, TableConstructor, Var,
};

use super::Printer;
use crate::separator::{INTERPOLATION_BRACE, OTHER, merges_after};

impl Printer {
    /// Names, numbers, and strings, half of all expressions, print inline
    /// where they're reached, which saves them a call to a function that
    /// saves every callee-saved register.
    #[expect(clippy::inline_always, reason = "an outlined copy would be the call it saves")]
    #[inline(always)]
    pub(super) fn print_expr(&mut self, expr: &Expression<'_>, slot: Slot) {
        match expr {
            Expression::Name(name) => self.name(name.text.as_str()),
            Expression::Number(literal) if !expr.needs_parens(slot) => self.print_number(literal),
            Expression::StringLiteral(literal) if !expr.needs_parens(slot) => {
                self.print_string(literal);
            }
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
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
            | Expression::Error(_) => self.print_other_expr(expr, slot),
        }
    }

    /// An expression in a slot it can't stand in bare prints in parentheses,
    /// and what's in them prints as if nothing were around it. Its operands
    /// take the slots that one gives them.
    ///
    /// Every level of nesting takes this frame, and each `print_expr` inlines
    /// into it, so a run and the Luau constructs print in frames of their own.
    fn print_other_expr(&mut self, expr: &Expression<'_>, outer: Slot) {
        let is_wrapped = expr.needs_parens(outer);
        let slot = if is_wrapped {
            self.piece("(");
            Slot::FREE
        } else {
            outer
        };
        match expr {
            Expression::Name(name) => self.name(name.text.as_str()),
            Expression::Number(literal) => self.print_number(literal),
            // Luau
            Expression::Integer(literal) => self.print_integer(literal),
            Expression::StringLiteral(literal) => self.print_string(literal),
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
                self.print_free_expr(&parenthesized.expr);
                self.piece(")");
            }
            Expression::TableConstructor(table) => self.print_table(table),
            Expression::BinaryOp(binary_op) => self.print_run(binary_op, slot),
            Expression::UnaryOp(unary_op) => {
                self.piece(unary_op.op.static_text());
                self.print_expr(&unary_op.operand, slot.unary_operand());
            }
            // Luau
            Expression::IfExpression(if_expr) => self.print_if_expr(if_expr),
            // Luau
            Expression::InterpolatedString(interpolated) => self.print_interpolated(interpolated),
            // Luau
            Expression::TypeCast(cast) => {
                self.print_expr(&cast.expr, Slot::CAST_OPERAND);
                self.piece("::");
                self.print_type(&cast.ty);
            }
            Expression::Error(_) => {}
        }
        if is_wrapped {
            self.piece(")");
        }
    }

    fn print_run(&mut self, binary_op: &BinaryOp<'_>, slot: Slot) {
        self.print_expr(&binary_op.first, slot.operand(binary_op, 0));
        for (index, link) in (1..).zip(&binary_op.links) {
            self.piece(link.op.static_text());
            self.print_expr(&link.operand, slot.operand(binary_op, index));
        }
    }

    /// Luau: a slot with anything after it takes parentheses, so nothing
    /// follows the `else` branch.
    fn print_if_expr(&mut self, if_expr: &IfExpression<'_>) {
        self.piece("if");
        self.print_free_expr(&if_expr.condition);
        self.piece("then");
        self.print_free_expr(&if_expr.then_expr);
        for clause in &if_expr.elseif_clauses {
            self.piece("elseif");
            self.print_free_expr(&clause.condition);
            self.piece("then");
            self.print_free_expr(&clause.expr);
        }
        self.piece("else");
        self.print_free_expr(&if_expr.else_expr);
    }

    fn print_interpolated(&mut self, interpolated: &InterpolatedString<'_>) {
        self.output.print_ascii_byte(b'`');
        for segment in &interpolated.segments {
            self.print_interpolated_text(&segment.text);
            self.output.print_ascii_byte(b'{');
            self.last = INTERPOLATION_BRACE;
            self.print_free_expr(&segment.expr);
            self.output.print_ascii_byte(b'}');
        }
        self.print_interpolated_text(&interpolated.tail);
        self.output.print_ascii_byte(b'`');
        self.last = OTHER;
    }

    /// An expression where nothing groups with it, as a list's items are,
    /// where a name or a literal never needs the check `print_expr` makes.
    #[expect(clippy::inline_always, reason = "see `print_expr`")]
    #[inline(always)]
    pub(super) fn print_free_expr(&mut self, expr: &Expression<'_>) {
        match expr {
            Expression::Name(name) => self.name(name.text.as_str()),
            Expression::Number(literal) => self.print_number(literal),
            Expression::StringLiteral(literal) => self.print_string(literal),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Integer(_)
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
            | Expression::Error(_) => self.print_other_expr(expr, Slot::FREE),
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
        self.print_expr(&chain.prefix, Slot::PREFIX);
        for suffix in &chain.suffixes {
            match suffix {
                Suffix::Field(name) => {
                    self.piece(".");
                    self.name(name.text.as_str());
                }
                Suffix::Index { index, .. } => {
                    self.piece("[");
                    self.print_free_expr(index);
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
                self.list(args, ",", Self::print_free_expr);
                self.piece(")");
            }
            FunctionArgs::TableConstructor(table) => self.print_table(table),
            FunctionArgs::StringLiteral(literal) => self.print_string(literal),
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
                self.print_free_expr(key);
                self.piece("]=");
                self.print_free_expr(value);
            }
            Field::Named { name, value } => {
                self.name(name.text.as_str());
                self.piece("=");
                self.print_free_expr(value);
            }
            Field::Positional(value) => self.print_free_expr(value),
        }
    }
}
