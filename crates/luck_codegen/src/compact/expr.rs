use luck_ast::{
    BinaryOp, Expression, Field, FieldAccess, FunctionArgs, FunctionCall, FunctionDef,
    IfExpression, IndexExpression, InterpolatedString, MAX_NESTING_DEPTH, ParenthesizedExpression,
    TableConstructor, TypeCast, TypeInstantiation, UnaryOp, Var, expr_starts_with_brace,
    regrouped_levels,
};
use luck_token::TokenKind;

use super::CompactPrinter;
use crate::separator::PieceClass;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the printer is one type with its emitters split by node family"
)]
impl CompactPrinter {
    pub(super) fn emit_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::Nil(_) => self.emit_fixed("nil"),
            Expression::False(_) => self.emit_fixed("false"),
            Expression::True(_) => self.emit_fixed("true"),
            Expression::VarArg(_) => self.emit_fixed("..."),
            Expression::Number(literal) | Expression::Integer(literal) => {
                self.emit_piece(&literal.text, PieceClass::Number);
            }
            Expression::StringLiteral(literal) => {
                self.emit_piece(&literal.text, PieceClass::Other);
            }
            Expression::FunctionDef(function) => self.emit_function_definition(function),
            Expression::Var(var) => self.emit_var(var),
            Expression::FunctionCall(call) => self.emit_function_call(call),
            Expression::Parenthesized(parenthesized) => {
                self.emit_parenthesized_expr(parenthesized);
            }
            Expression::TableConstructor(table) => self.emit_table_constructor(table),
            Expression::BinaryOp(binary_op) => self.emit_binary_op(binary_op),
            Expression::UnaryOp(unary_op) => self.emit_unary_op(unary_op),
            // Luau
            Expression::IfExpression(if_expr) => self.emit_if_expr(if_expr),
            // Luau
            Expression::InterpolatedString(string) => self.emit_interpolated_string(string),
            // Luau
            Expression::TypeCast(cast) => self.emit_type_cast(cast),
            // Luau
            Expression::TypeInstantiation(instantiation) => {
                self.emit_type_instantiation(instantiation);
            }
            Expression::Error(_) => {}
        }
    }

    pub(super) fn emit_var(&mut self, var: &Var) {
        match var {
            Var::Name(token) => self.emit_token(token),
            Var::Index(index) => self.emit_index_expr(index),
            Var::FieldAccess(field) => self.emit_field_access(field),
        }
    }

    fn emit_function_definition(&mut self, function_definition: &FunctionDef) {
        self.emit_function_attributes(&function_definition.attributes);
        self.emit_fixed("function");
        self.emit_function_body(&function_definition.body);
    }

    pub(super) fn emit_function_call(&mut self, call: &FunctionCall) {
        if self.descend() {
            self.emit_expr(&call.callee);
            if let Some(method_name) = &call.method {
                self.emit_fixed(":");
                self.emit_token(method_name);
            }
            if let Some(type_args) = &call.explicit_type_args {
                self.emit_fixed("<<");
                self.emit_punctuated(&type_args.args, ",", Self::emit_type);
                self.emit_fixed(">>");
            }
            self.emit_function_args(&call.args);
        }
        self.ascend();
    }

    fn emit_function_args(&mut self, function_args: &FunctionArgs) {
        match function_args {
            FunctionArgs::Parenthesized { args, .. } => {
                self.emit_fixed("(");
                self.emit_punctuated(args, ",", Self::emit_expr);
                self.emit_fixed(")");
            }
            FunctionArgs::TableConstructor(table) => self.emit_table_constructor(table),
            FunctionArgs::StringLiteral(literal) => {
                self.emit_piece(&literal.text, PieceClass::Other);
            }
        }
    }

    fn emit_parenthesized_expr(&mut self, parenthesized: &ParenthesizedExpression) {
        if self.descend() {
            self.emit_fixed("(");
            self.emit_expr(&parenthesized.expr);
            self.emit_fixed(")");
        }
        self.ascend();
    }

    fn emit_table_constructor(&mut self, table: &TableConstructor) {
        if self.descend() {
            self.emit_fixed("{");
            // A trailing separator the source carried is dropped; it costs bytes
            // and changes nothing.
            self.emit_joined(&table.fields.items, ",", Self::emit_field);
            self.emit_fixed("}");
        }
        self.ascend();
    }

    fn emit_field(&mut self, field: &Field) {
        match field {
            Field::Bracketed { key, value, .. } => {
                self.emit_fixed("[");
                self.emit_expr(key);
                self.emit_fixed("]");
                self.emit_fixed("=");
                self.emit_expr(value);
            }
            Field::Named { name, value, .. } => {
                self.emit_token(name);
                self.emit_fixed("=");
                self.emit_expr(value);
            }
            Field::Positional { value, .. } => {
                self.emit_expr(value);
            }
        }
    }

    fn emit_index_expr(&mut self, index: &IndexExpression) {
        if self.descend() {
            self.emit_expr(&index.prefix);
            self.emit_fixed("[");
            self.emit_expr(&index.index);
            self.emit_fixed("]");
        }
        self.ascend();
    }

    fn emit_field_access(&mut self, field: &FieldAccess) {
        if self.descend() {
            self.emit_expr(&field.prefix);
            self.emit_fixed(".");
            self.emit_token(&field.name);
        }
        self.ascend();
    }

    fn emit_binary_op(&mut self, binary_op: &BinaryOp) {
        if self.descend() {
            // An operand that regroups when read back counts at the depth it
            // ends up at.
            let (lhs_levels, rhs_levels) = regrouped_levels(binary_op);
            self.emit_regrouped(&binary_op.lhs, lhs_levels);
            self.emit_fixed(binary_op.op.static_text());
            self.emit_regrouped(&binary_op.rhs, rhs_levels);
        }
        self.ascend();
    }

    fn emit_regrouped(&mut self, operand: &Expression, levels: usize) {
        self.depth = self.depth.saturating_add(levels);
        if self.depth > MAX_NESTING_DEPTH {
            self.is_too_deep = true;
        } else {
            self.emit_expr(operand);
        }
        self.depth = self.depth.saturating_sub(levels);
    }

    fn emit_unary_op(&mut self, unary_op: &UnaryOp) {
        if self.descend() {
            self.emit_fixed(unary_op.op.static_text());
            self.emit_expr(&unary_op.operand);
        }
        self.ascend();
    }

    fn emit_if_expr(&mut self, if_expr: &IfExpression) {
        if self.descend() {
            self.emit_fixed("if");
            self.emit_expr(&if_expr.condition);
            self.emit_fixed("then");
            self.emit_expr(&if_expr.then_expr);
            for clause in &if_expr.elseif_clauses {
                self.emit_fixed("elseif");
                self.emit_expr(&clause.condition);
                self.emit_fixed("then");
                self.emit_expr(&clause.expr);
            }
            self.emit_fixed("else");
            self.emit_expr(&if_expr.else_expr);
        }
        self.ascend();
    }

    fn emit_interpolated_string(&mut self, interpolated: &InterpolatedString) {
        if self.descend() {
            // The braces follow expression presence, not token kind: a plain
            // `` `text` `` lexes as a Begin("") + End(text) pair with no
            // expression and must print without braces.
            self.output.print_ascii_byte(b'`');
            for segment in &interpolated.segments {
                if let TokenKind::InterpolatedStringBegin(text)
                | TokenKind::InterpolatedStringMiddle(text)
                | TokenKind::InterpolatedStringEnd(text) = &segment.literal.kind
                {
                    self.output.print_str(text);
                }
                if let Some(expr) = &segment.expr {
                    self.output.print_ascii_byte(b'{');
                    // `{{` is a parse error in Luau: an expression starting with a
                    // table constructor needs a space after the interpolation
                    // opener.
                    if expr_starts_with_brace(expr) {
                        self.output.print_ascii_byte(b' ');
                    }
                    self.previous = PieceClass::Other;
                    self.emit_expr(expr);
                    self.output.print_ascii_byte(b'}');
                }
            }
            self.output.print_ascii_byte(b'`');
            self.previous = PieceClass::Other;
        }
        self.ascend();
    }

    fn emit_type_cast(&mut self, cast: &TypeCast) {
        if self.descend() {
            self.emit_expr(&cast.expr);
            self.emit_fixed("::");
            self.emit_type(&cast.type_annotation);
        }
        self.ascend();
    }

    fn emit_type_instantiation(&mut self, instantiation: &TypeInstantiation) {
        if self.descend() {
            self.emit_expr(&instantiation.expr);
            self.emit_fixed("<<");
            self.emit_punctuated(&instantiation.type_args.args, ",", Self::emit_type);
            self.emit_fixed(">>");
        }
        self.ascend();
    }
}
