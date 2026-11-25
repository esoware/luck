use luck_ast::{
    BinaryOp, Expression, Field, FunctionArgs, FunctionDef, IfExpression, InterpolatedString,
    MethodCall, Slot, Suffix, SuffixedExpression, TableConstructor, TypeCast, UnaryOp, Var,
};
use luck_token::{BinOp, UnOp};

use super::Printer;
use crate::separator::{INTERPOLATION_BRACE, OTHER, merges_after};

impl Printer {
    /// An expression where nothing groups with it, as a list's items are,
    /// which never takes parentheses, so every check of `print_expr` folds
    /// away.
    #[expect(clippy::inline_always, reason = "see `print_expr`")]
    #[inline(always)]
    pub(super) fn print_free_expr(&mut self, expr: &Expression<'_>) {
        self.print_expr(expr, Slot::FREE);
    }

    /// An expression in a slot it can't stand in bare prints in parentheses,
    /// and what's in them prints as if nothing were around it. Its operands
    /// take the slots `slot` gives them.
    ///
    /// Each kind goes straight to its own printer from where it's reached,
    /// asking `slot` about itself on the way, and a name, a literal, or a
    /// keyword prints right there, so none takes a call to a function that
    /// only dispatches it.
    #[expect(clippy::inline_always, reason = "an outlined copy would be the call it saves")]
    #[inline(always)]
    fn print_expr(&mut self, expr: &Expression<'_>, slot: Slot) {
        match expr {
            Expression::Name(name) => self.name(name.text.as_str()),
            Expression::Number(literal) if !slot.wraps_atom() => self.print_number(literal),
            Expression::StringLiteral(literal) if !slot.wraps_atom() => {
                self.print_string(literal);
            }
            Expression::Nil(_) if !slot.wraps_atom() => self.piece("nil"),
            Expression::False(_) if !slot.wraps_atom() => self.piece("false"),
            Expression::True(_) if !slot.wraps_atom() => self.piece("true"),
            // `.....` reads as `...` `..`, so nothing merges with a vararg.
            Expression::VarArg(_) if !slot.wraps_atom() => {
                self.emit(b"...", merges_after(b'.'), OTHER);
            }
            Expression::FunctionDef(function) if !slot.wraps_atom() => {
                self.print_function_def(function);
            }
            Expression::TableConstructor(table) if !slot.wraps_atom() => self.print_table(table),
            Expression::InterpolatedString(interpolated) if !slot.wraps_atom() => {
                self.print_interpolated(interpolated);
            }
            Expression::BinaryOp(binary_op) if !slot.wraps_run(binary_op) => {
                self.print_run(binary_op, slot);
            }
            Expression::UnaryOp(unary_op) if !slot.wraps_unary() => {
                self.print_unary_op(unary_op, slot);
            }
            Expression::IfExpression(if_expr) if !slot.wraps_if() => self.print_if_expr(if_expr),
            Expression::TypeCast(cast) if !slot.wraps_cast(cast) => self.print_type_cast(cast),
            Expression::Suffixed(chain) => self.print_suffixed(chain),
            Expression::Parenthesized(parenthesized) => {
                self.print_parenthesized(&parenthesized.expr);
            }
            Expression::Error(_) => {}
            Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::TableConstructor(_)
            | Expression::InterpolatedString(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_) => self.print_parenthesized(expr),
        }
    }

    fn print_function_def(&mut self, function: &FunctionDef<'_>) {
        if !function.attributes.is_empty() {
            self.print_function_attributes(&function.attributes);
        }
        self.piece("function");
        self.print_function_body(&function.body);
    }

    fn print_parenthesized(&mut self, expr: &Expression<'_>) {
        self.piece("(");
        self.print_free_expr(expr);
        self.piece(")");
    }

    fn print_run(&mut self, binary_op: &BinaryOp<'_>, slot: Slot) {
        self.print_expr(&binary_op.first, slot.operand(binary_op, 0));
        for (index, link) in (1..).zip(&binary_op.links) {
            self.binary_operator(link.op);
            self.print_expr(&link.operand, slot.operand(binary_op, index));
        }
    }

    /// Each operator prints from an arm of its own, as a piece whose text is
    /// a constant, so what it merges with is one too.
    fn binary_operator(&mut self, op: BinOp) {
        match op {
            BinOp::Add => self.piece("+"),
            BinOp::Sub => self.piece("-"),
            BinOp::Mul => self.piece("*"),
            BinOp::Div => self.piece("/"),
            BinOp::FloorDiv => self.piece("//"),
            BinOp::Mod => self.piece("%"),
            BinOp::Pow => self.piece("^"),
            BinOp::Concat => self.piece(".."),
            BinOp::Eq => self.piece("=="),
            BinOp::Ne => self.piece("~="),
            BinOp::Lt => self.piece("<"),
            BinOp::Le => self.piece("<="),
            BinOp::Gt => self.piece(">"),
            BinOp::Ge => self.piece(">="),
            BinOp::And => self.piece("and"),
            BinOp::Or => self.piece("or"),
        }
    }

    fn print_unary_op(&mut self, unary_op: &UnaryOp<'_>, slot: Slot) {
        match unary_op.op {
            UnOp::Not => self.piece("not"),
            UnOp::Neg => self.piece("-"),
            UnOp::Len => self.piece("#"),
        }
        self.print_expr(&unary_op.operand, slot.unary_operand());
    }

    /// A slot with anything after it takes parentheses, so nothing follows
    /// the `else` branch.
    fn print_if_expr(&mut self, if_expr: &IfExpression<'_>) {
        self.piece("if");
        self.print_condition(if_expr.binding.as_deref(), &if_expr.condition);
        self.piece("then");
        self.print_free_expr(&if_expr.then_expr);
        for clause in &if_expr.elseif_clauses {
            self.piece("elseif");
            self.print_condition(clause.binding.as_deref(), &clause.condition);
            self.piece("then");
            self.print_free_expr(&clause.expr);
        }
        self.piece("else");
        self.print_free_expr(&if_expr.else_expr);
    }

    fn print_interpolated(&mut self, interpolated: &InterpolatedString<'_>) {
        self.output.print_byte(b'`');
        for segment in &interpolated.segments {
            self.print_interpolated_text(&segment.text);
            self.output.print_byte(b'{');
            self.last = INTERPOLATION_BRACE;
            self.print_free_expr(&segment.expr);
            self.output.print_byte(b'}');
        }
        self.print_interpolated_text(&interpolated.tail);
        self.output.print_byte(b'`');
        self.last = OTHER;
    }

    fn print_type_cast(&mut self, cast: &TypeCast<'_>) {
        self.print_expr(&cast.expr, Slot::CAST_OPERAND);
        self.piece("::");
        self.print_type(&cast.ty);
    }

    pub(super) fn print_var(&mut self, var: &Var<'_>) {
        match var {
            Var::Name(name) => self.name(name.text.as_str()),
            Var::Suffixed(chain) => self.print_suffixed(chain),
        }
    }

    #[inline]
    pub(super) fn print_suffixed(&mut self, chain: &SuffixedExpression<'_>) {
        self.print_expr(&chain.prefix, Slot::PREFIX);
        for suffix in &chain.suffixes {
            match suffix {
                Suffix::Field(name) => {
                    self.piece(".");
                    self.name(name.text.as_str());
                }
                Suffix::Index { index, .. } => self.print_index(index),
                Suffix::Call(args) => self.print_args(args),
                Suffix::MethodCall(call) => self.print_method_call(call),
                Suffix::TypeInstantiation { type_args, .. } => {
                    self.print_type_instantiation(type_args);
                }
            }
        }
    }

    fn print_index(&mut self, index: &Expression<'_>) {
        self.piece("[");
        self.print_free_expr(index);
        self.piece("]");
    }

    fn print_method_call(&mut self, call: &MethodCall<'_>) {
        self.piece(":");
        self.name(call.name.text.as_str());
        if let Some(type_args) = &call.type_args {
            self.print_type_instantiation(type_args);
        }
        self.print_args(&call.args);
    }

    fn print_args(&mut self, function_args: &FunctionArgs<'_>) {
        match function_args {
            FunctionArgs::Parenthesized { args, .. } => {
                self.piece("(");
                self.list(args, Self::print_free_expr);
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
        self.list(&table.fields, Self::print_field);
        self.piece("}");
    }

    fn print_field(&mut self, field: &Field<'_>) {
        match field {
            Field::Bracketed { key, value, .. } => self.print_bracketed_field(key, value),
            Field::Named { name, value } => self.print_named_field(name.text.as_str(), value),
            Field::Positional(value) => self.print_free_expr(value),
        }
    }

    fn print_bracketed_field(&mut self, key: &Expression<'_>, value: &Expression<'_>) {
        self.piece("[");
        self.print_free_expr(key);
        self.piece("]=");
        self.print_free_expr(value);
    }

    fn print_named_field(&mut self, name: &str, value: &Expression<'_>) {
        self.name(name);
        self.piece("=");
        self.print_free_expr(value);
    }
}
