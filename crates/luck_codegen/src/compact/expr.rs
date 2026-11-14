use luck_ast::{
    BinaryOp, Expression, Field, FunctionArgs, FunctionDef, IfExpression, InterpolatedString,
    ParenthesizedExpression, Suffix, SuffixedExpression, TableConstructor, TypeCast, UnaryOp, Var,
    check_interpolated_string, error_node, expr_starts_with_brace, integer_spelling,
    number_spelling, string_spelling,
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
            Expression::Number(literal) => {
                self.emit_leaf(number_spelling(literal), PieceClass::Number);
            }
            // Luau
            Expression::Integer(literal) => {
                self.emit_leaf(integer_spelling(literal), PieceClass::Number);
            }
            Expression::StringLiteral(literal) => {
                self.emit_leaf(string_spelling(literal), PieceClass::Other);
            }
            Expression::FunctionDef(function) => self.emit_function_definition(function),
            Expression::Name(token) => self.emit_name(token),
            Expression::Suffixed(chain) => self.emit_suffixed(chain),
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
            Expression::Error(span) => self.refuse(error_node(*span)),
        }
    }

    pub(super) fn emit_var(&mut self, var: &Var) {
        match var {
            Var::Name(token) => self.emit_name(token),
            Var::Suffixed(chain) => self.emit_suffixed(chain),
        }
    }

    fn emit_function_definition(&mut self, function_definition: &FunctionDef) {
        self.emit_function_attributes(&function_definition.attributes);
        self.emit_fixed("function");
        self.emit_function_body(&function_definition.body);
    }

    pub(super) fn emit_suffixed(&mut self, chain: &SuffixedExpression) {
        if self.descend() {
            self.emit_expr(&chain.prefix);
            for suffix in &chain.suffixes {
                self.emit_suffix(suffix);
            }
        }
        self.ascend();
    }

    fn emit_suffix(&mut self, suffix: &Suffix) {
        match suffix {
            Suffix::Field(name) => {
                self.emit_fixed(".");
                self.emit_name(name);
            }
            Suffix::Index { index, .. } => {
                self.emit_fixed("[");
                self.emit_expr(index);
                self.emit_fixed("]");
            }
            Suffix::Call(call) => {
                if let Some(method_name) = &call.method {
                    self.emit_fixed(":");
                    self.emit_name(method_name);
                }
                if let Some(type_args) = &call.explicit_type_args {
                    self.emit_fixed("<<");
                    self.emit_punctuated(&type_args.args, ",", Self::emit_type);
                    self.emit_fixed(">>");
                }
                self.emit_function_args(&call.args);
            }
            // Luau
            Suffix::TypeInstantiation { type_args, .. } => {
                self.emit_fixed("<<");
                self.emit_punctuated(&type_args.args, ",", Self::emit_type);
                self.emit_fixed(">>");
            }
        }
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
                self.emit_leaf(string_spelling(literal), PieceClass::Other);
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
                self.emit_name(name);
                self.emit_fixed("=");
                self.emit_expr(value);
            }
            Field::Positional { value, .. } => {
                self.emit_expr(value);
            }
        }
    }

    fn emit_binary_op(&mut self, binary_op: &BinaryOp) {
        if self.descend() {
            self.emit_expr(&binary_op.first);
            for link in &binary_op.links {
                self.emit_fixed(link.op.static_text());
                self.emit_expr(&link.operand);
            }
        }
        self.ascend();
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
        if let Err(error) = check_interpolated_string(interpolated) {
            self.refuse(error);
        }
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
}
