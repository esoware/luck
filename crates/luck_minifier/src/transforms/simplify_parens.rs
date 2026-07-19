use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::transform::AstTransform;
use luck_token::token::{Token, TokenKind};

use crate::tokens::default_span as sp;

/// Remove unnecessary parentheses using operator precedence and expression type analysis.
pub fn simplify(block: Block) -> Block {
    ParenSimplifier.transform_block(block)
}

struct ParenSimplifier;

impl AstTransform for ParenSimplifier {
    fn transform_expression(&mut self, expr: Expression) -> Expression {
        let expr = self.walk_expression(expr);

        match expr {
            Expression::Parenthesized(paren) => {
                if can_remove_parens(&paren.expr) {
                    paren.expr
                } else {
                    make_clean_parens(paren.expr)
                }
            }
            Expression::BinaryOp(mut binop) => {
                if let Expression::Parenthesized(paren) = binop.left {
                    if can_unwrap_in_binop_lhs(&paren.expr, &binop.op) {
                        binop.left = paren.expr;
                    } else {
                        binop.left = make_clean_parens(paren.expr);
                    }
                }
                if let Expression::Parenthesized(paren) = binop.right {
                    if can_unwrap_in_binop_rhs(&paren.expr, &binop.op) {
                        binop.right = paren.expr;
                    } else {
                        binop.right = make_clean_parens(paren.expr);
                    }
                }
                Expression::BinaryOp(binop)
            }
            Expression::UnaryOp(mut unop) => {
                if let Expression::Parenthesized(paren) = unop.operand {
                    if can_unwrap_in_unary(&paren.expr) {
                        unop.operand = paren.expr;
                    } else {
                        unop.operand = make_clean_parens(paren.expr);
                    }
                }
                Expression::UnaryOp(unop)
            }
            other => other,
        }
    }

    // f("str") -> f"str", f({...}) -> f{...}
    fn walk_function_args(&mut self, args: FunctionArgs) -> FunctionArgs {
        match args {
            FunctionArgs::Parenthesized { parens, args } => {
                if args.len() == 1 {
                    let items: Vec<_> = args.iter().collect();
                    match items[0] {
                        Expression::StringLiteral(token) => {
                            return FunctionArgs::StringLiteral(token.clone());
                        }
                        Expression::TableConstructor(table) => {
                            return FunctionArgs::TableConstructor(Box::new(
                                self.walk_table_constructor(*table.clone()),
                            ));
                        }
                        _ => {}
                    }
                }
                let args = self.walk_punctuated_exprs(args);
                FunctionArgs::Parenthesized { parens, args }
            }
            FunctionArgs::TableConstructor(table) => {
                FunctionArgs::TableConstructor(Box::new(self.walk_table_constructor(*table)))
            }
            other => other,
        }
    }

    fn transform_var(&mut self, var: Var) -> Var {
        match var {
            // Prefix parens required for `("str"):method()`
            Var::FieldAccess(mut field_access) => {
                field_access.prefix = match field_access.prefix {
                    Expression::Parenthesized(paren) => {
                        let transformed = self.transform_expression(paren.expr);
                        make_clean_parens(transformed)
                    }
                    other => self.transform_expression(other),
                };
                Var::FieldAccess(field_access)
            }
            Var::Index(mut index_expr) => {
                index_expr.prefix = match index_expr.prefix {
                    Expression::Parenthesized(paren) => {
                        let transformed = self.transform_expression(paren.expr);
                        make_clean_parens(transformed)
                    }
                    other => self.transform_expression(other),
                };
                index_expr.index = self.transform_expression(index_expr.index);
                Var::Index(index_expr)
            }
            other => self.walk_var(other),
        }
    }

    fn walk_function_call(&mut self, mut call: FunctionCall) -> FunctionCall {
        call.callee = match call.callee {
            Expression::Parenthesized(paren) => {
                let transformed = self.transform_expression(paren.expr);
                make_clean_parens(transformed)
            }
            other => self.transform_expression(other),
        };
        call.args = self.walk_function_args(call.args);
        call
    }
}

fn can_remove_parens(inner: &Expression) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::Var(_) => true,
        Expression::TableConstructor(_) => true,
        Expression::Parenthesized(_) => true,
        Expression::FunctionDef(_) => true,
        // Function calls - NOT safe (changes multi-return behavior)
        Expression::FunctionCall(_) => false,
        Expression::BinaryOp(_) | Expression::UnaryOp(_) => false,
        Expression::IfExpression(_) => false,
        _ => false,
    }
}

fn binop_precedence(op: &Token) -> u8 {
    match op.kind {
        TokenKind::Or => 1,
        TokenKind::And => 2,
        TokenKind::Less
        | TokenKind::Greater
        | TokenKind::LessEqual
        | TokenKind::GreaterEqual
        | TokenKind::EqualEqual
        | TokenKind::TildeEqual => 3,
        TokenKind::Pipe => 4,
        TokenKind::Tilde => 5,
        TokenKind::Ampersand => 6,
        TokenKind::ShiftLeft | TokenKind::ShiftRight => 7,
        TokenKind::DotDot => 8,
        TokenKind::Plus | TokenKind::Minus => 9,
        TokenKind::Star | TokenKind::Slash | TokenKind::FloorDiv | TokenKind::Percent => 10,
        TokenKind::Caret => 12,
        _ => 0,
    }
}

fn is_right_associative(op: &Token) -> bool {
    matches!(op.kind, TokenKind::Caret | TokenKind::DotDot)
}

fn can_unwrap_in_binop_lhs(inner: &Expression, outer: &Token) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Var(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => true,
        Expression::FunctionCall(_) => false,
        // Unary ops have lower precedence than ^, so (-a)^b needs parens
        Expression::UnaryOp(_) => !matches!(outer.kind, TokenKind::Caret),
        Expression::BinaryOp(binop) => {
            let inner_prec = binop_precedence(&binop.op);
            let outer_prec = binop_precedence(outer);
            if inner_prec > outer_prec {
                return true;
            }
            if inner_prec == outer_prec && !is_right_associative(outer) {
                return true;
            }
            false
        }
        _ => false,
    }
}

fn can_unwrap_in_binop_rhs(inner: &Expression, outer: &Token) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Var(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => true,
        Expression::FunctionCall(_) => false,
        Expression::UnaryOp(_) => true,
        Expression::BinaryOp(binop) => {
            let inner_prec = binop_precedence(&binop.op);
            let outer_prec = binop_precedence(outer);
            if inner_prec > outer_prec {
                return true;
            }
            if inner_prec == outer_prec && is_right_associative(outer) {
                return true;
            }
            false
        }
        _ => false,
    }
}

fn can_unwrap_in_unary(inner: &Expression) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Var(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => true,
        Expression::UnaryOp(_) => true,
        // NOT safe: -a+b != -(a+b)
        Expression::BinaryOp(_) => false,
        Expression::FunctionCall(_) => false,
        _ => false,
    }
}

fn make_clean_parens(expr: Expression) -> Expression {
    Expression::Parenthesized(Box::new(ParenExpression {
        span: sp(),
        parens: ContainedSpan {
            open: Token::new(TokenKind::LeftParen, sp()),
            close: Token::new(TokenKind::RightParen, sp()),
        },
        expr,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = simplify(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn removes_parens_number_in_assignment() {
        let r = apply("local x = (42)\n");
        assert!(!r.contains("(42)"), "Got: {r}");
    }

    #[test]
    fn removes_parens_variable() {
        let r = apply("local x = (y)\n");
        assert!(!r.contains("(y)"), "Got: {r}");
    }

    #[test]
    fn keeps_parens_func_call() {
        let r = apply("local x = (foo())\n");
        assert!(r.contains("(foo())"), "Multi-return parens removed: {r}");
    }

    #[test]
    fn higher_precedence_inner_unwraps() {
        let r = apply("return (a * b) + c\n");
        assert!(
            !r.contains("(a*b)") && !r.contains("(a * b)"),
            "Higher prec inner should unwrap, got: {r}"
        );
    }

    #[test]
    fn lower_precedence_rhs_keeps_parens() {
        let r = apply("return a * (b + c)\n");
        assert!(r.contains("("), "Lower prec rhs must keep parens, got: {r}");
    }

    #[test]
    fn keeps_parens_lower_prec_in_multiply() {
        let r = apply("return (1 + 2) * 3\n");
        assert!(
            r.contains("("),
            "Lower prec (1+2) must keep parens in multiply: {r}"
        );
    }

    #[test]
    fn removes_parens_around_variable_in_multiply() {
        let r = apply("return (x) * 3\n");
        assert!(
            !r.contains("(x)"),
            "Parens around variable should be removed: {r}"
        );
    }

    #[test]
    fn keeps_parens_unary_in_exponentiation() {
        let r = apply("return (-a) ^ b\n");
        assert!(r.contains("(-a)"), "Parens required for (-a)^b, got: {r}");
    }

    #[test]
    fn strips_parens_on_single_arg_string_call() {
        let r = apply("print(\"hello\")\n");
        assert!(
            r.contains("print\"hello\""),
            "Single string arg should strip parens: {r}"
        );
    }

    #[test]
    fn strips_parens_on_single_arg_table_call() {
        let r = apply("foo({1, 2})\n");
        assert!(
            r.contains("foo{"),
            "Single table arg should strip parens: {r}"
        );
    }

    #[test]
    fn keeps_parens_left_concat_in_concat() {
        // (a .. b) .. c != a .. b .. c because concat is right-associative
        let r = apply("return (a .. b) .. c\n");
        assert!(
            r.contains("("),
            "Left-associative concat grouping needs parens: {r}"
        );
    }

    #[test]
    fn removes_parens_right_concat_in_concat() {
        // a .. (b .. c) == a .. b .. c because concat is right-associative
        let r = apply("return a .. (b .. c)\n");
        assert!(
            !r.contains("("),
            "Right-associative concat parens are redundant: {r}"
        );
    }

    #[test]
    fn keeps_parens_func_call_in_binop() {
        // (f()) + 1 - parens change multi-return truncation
        let r = apply("return (f()) + 1\n");
        assert!(
            r.contains("(f())"),
            "Parens around call in binop must be kept: {r}"
        );
    }
}
