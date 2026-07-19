use luck_ast::expr::*;
use luck_ast::shared::Block;
use luck_ast::transform::AstTransform;
use luck_token::LuaVersion;
use luck_token::token::{Token, TokenKind};

use crate::expr::{
    LuaNumber, decode_string_literal, encode_string_literal, extract_boolean, extract_lua_number,
    is_nil,
};
use crate::tokens::default_span as sp;

/// Evaluate compile-time constant expressions (arithmetic, string concat, boolean logic).
/// Version-aware: Lua 5.3+ integer/float subtypes are preserved exactly.
pub fn fold(block: Block, version: LuaVersion) -> Block {
    ConstFolder {
        int_subtype: version.has_integer_subtype(),
    }
    .transform_block(block)
}

struct ConstFolder {
    int_subtype: bool,
}

impl AstTransform for ConstFolder {
    fn transform_expression(&mut self, expr: Expression) -> Expression {
        let expr = self.walk_expression(expr);

        match expr {
            Expression::BinaryOp(binop) => {
                if let Some(result) =
                    try_fold_binary(&binop.left, &binop.op, &binop.right, self.int_subtype)
                {
                    return result;
                }
                Expression::BinaryOp(binop)
            }
            Expression::UnaryOp(unop) => {
                if let Some(result) = try_fold_unary(&unop.op, &unop.operand, self.int_subtype) {
                    return result;
                }
                Expression::UnaryOp(unop)
            }
            other => other,
        }
    }
}

/// An integer whose f64 image is exact - beyond this a float comparison
/// against an int literal silently loses precision, so we refuse to fold.
fn int_fits_f64(value: i64) -> bool {
    value.abs() <= (1i64 << 53)
}

fn fold_numeric(l: LuaNumber, op: &TokenKind, r: LuaNumber) -> Option<LuaNumber> {
    use LuaNumber::{Float, Int};
    match (l, r) {
        (Int(a), Int(b)) => match op {
            // Integer ops wrap, matching Lua 5.3+ exactly.
            TokenKind::Plus => Some(Int(a.wrapping_add(b))),
            TokenKind::Minus => Some(Int(a.wrapping_sub(b))),
            TokenKind::Star => Some(Int(a.wrapping_mul(b))),
            // `/` and `^` always produce floats.
            TokenKind::Slash => {
                if b != 0 {
                    Some(Float(a as f64 / b as f64))
                } else {
                    None
                }
            }
            TokenKind::Caret => Some(Float((a as f64).powf(b as f64))),
            // Integer % 0 and // 0 raise at runtime - never fold.
            TokenKind::Percent => {
                if b != 0 {
                    Some(Int(floored_mod(a, b)))
                } else {
                    None
                }
            }
            TokenKind::FloorDiv => {
                if b != 0 {
                    Some(Int(floored_div(a, b)?))
                } else {
                    None
                }
            }
            _ => None,
        },
        (Float(a), Float(b)) => fold_float(a, op, b),
        (Int(a), Float(b)) => {
            if !int_fits_f64(a) {
                return None;
            }
            fold_float(a as f64, op, b)
        }
        (Float(a), Int(b)) => {
            if !int_fits_f64(b) {
                return None;
            }
            fold_float(a, op, b as f64)
        }
    }
}

fn floored_mod(a: i64, b: i64) -> i64 {
    let m = a.wrapping_rem(b);
    if m != 0 && (m < 0) != (b < 0) {
        m + b
    } else {
        m
    }
}

fn floored_div(a: i64, b: i64) -> Option<i64> {
    // i64::MIN / -1 overflows; Lua wraps, but the literal form of the
    // result can't round-trip (see make_int_expr) - skip.
    let quotient = a.checked_div(b)?;
    let remainder = a.wrapping_rem(b);
    if remainder != 0 && (remainder < 0) != (b < 0) {
        quotient.checked_sub(1)
    } else {
        Some(quotient)
    }
}

fn fold_float(a: f64, op: &TokenKind, b: f64) -> Option<LuaNumber> {
    let value = match op {
        TokenKind::Plus => a + b,
        TokenKind::Minus => a - b,
        TokenKind::Star => a * b,
        TokenKind::Slash if b != 0.0 => a / b,
        TokenKind::Percent if b != 0.0 => {
            // Lua uses floored modulo: a % b = a - floor(a/b) * b
            a - (a / b).floor() * b
        }
        TokenKind::Caret => a.powf(b),
        TokenKind::FloorDiv if b != 0.0 => (a / b).floor(),
        _ => return None,
    };
    Some(LuaNumber::Float(value))
}

fn compare_numbers(l: LuaNumber, op: &TokenKind, r: LuaNumber) -> Option<bool> {
    use LuaNumber::{Float, Int};
    use std::cmp::Ordering;
    let ordering = match (l, r) {
        (Int(a), Int(b)) => a.cmp(&b),
        (Float(a), Float(b)) => a.partial_cmp(&b)?,
        // Mixed int/float compares mathematically in Lua; via f64 only
        // when the int converts exactly.
        (Int(a), Float(b)) => {
            if !int_fits_f64(a) {
                return None;
            }
            (a as f64).partial_cmp(&b)?
        }
        (Float(a), Int(b)) => {
            if !int_fits_f64(b) {
                return None;
            }
            a.partial_cmp(&(b as f64))?
        }
    };
    Some(match op {
        TokenKind::Less => ordering == Ordering::Less,
        TokenKind::Greater => ordering == Ordering::Greater,
        TokenKind::LessEqual => ordering != Ordering::Greater,
        TokenKind::GreaterEqual => ordering != Ordering::Less,
        TokenKind::EqualEqual => ordering == Ordering::Equal,
        TokenKind::TildeEqual => ordering != Ordering::Equal,
        _ => return None,
    })
}

/// A call or `...` yields multiple values in tail position; when a fold
/// substitutes it for an `and`/`or` expression (which always yields one
/// value) it must be parenthesized to keep the truncation.
fn truncate_multi_value(expr: &Expression) -> Expression {
    match expr {
        Expression::FunctionCall(_) | Expression::VarArg(_) => {
            Expression::Parenthesized(Box::new(ParenExpression {
                span: sp(),
                parens: luck_ast::shared::ContainedSpan {
                    open: Token::new(TokenKind::LeftParen, sp()),
                    close: Token::new(TokenKind::RightParen, sp()),
                },
                expr: expr.clone(),
            }))
        }
        _ => expr.clone(),
    }
}

fn extract_string_bytes(expr: &Expression) -> Option<Vec<u8>> {
    match expr {
        Expression::StringLiteral(token) => {
            if let TokenKind::StringLiteral(ref raw) = token.kind {
                decode_string_literal(raw)
            } else {
                None
            }
        }
        Expression::Parenthesized(paren) => extract_string_bytes(&paren.expr),
        _ => None,
    }
}

fn try_fold_binary(
    lhs: &Expression,
    op: &Token,
    rhs: &Expression,
    int_subtype: bool,
) -> Option<Expression> {
    if let (Some(l), Some(r)) = (
        extract_lua_number(lhs, int_subtype),
        extract_lua_number(rhs, int_subtype),
    ) {
        if let Some(value) = fold_numeric(l, &op.kind, r) {
            if let Some(folded) = make_lua_number_expr(value, int_subtype) {
                return Some(folded);
            }
        }
        if let Some(value) = compare_numbers(l, &op.kind, r) {
            return Some(make_boolean_expr(value));
        }
    }

    // Strings fold on their DECODED byte values - comparing or joining
    // raw escaped text conflates `"\65"` with `"\\65"` and worse.
    if let (Some(l), Some(r)) = (extract_string_bytes(lhs), extract_string_bytes(rhs)) {
        match op.kind {
            TokenKind::EqualEqual => return Some(make_boolean_expr(l == r)),
            TokenKind::TildeEqual => return Some(make_boolean_expr(l != r)),
            TokenKind::DotDot => {
                let mut joined = l;
                joined.extend_from_slice(&r);
                let raw = encode_string_literal(&joined);
                return Some(Expression::StringLiteral(Token::new(
                    TokenKind::StringLiteral(raw.into()),
                    sp(),
                )));
            }
            _ => {}
        }
    }

    if let (Some(l), Some(r)) = (extract_boolean(lhs), extract_boolean(rhs)) {
        match op.kind {
            TokenKind::EqualEqual => return Some(make_boolean_expr(l == r)),
            TokenKind::TildeEqual => return Some(make_boolean_expr(l != r)),
            _ => {}
        }
    }

    match op.kind {
        TokenKind::And => {
            if let Some(b) = extract_boolean(lhs) {
                if !b {
                    return Some(make_boolean_expr(false));
                }
                return Some(truncate_multi_value(rhs));
            }
            if is_nil(lhs) {
                return Some(lhs.clone());
            }
        }
        TokenKind::Or => {
            if let Some(b) = extract_boolean(lhs) {
                if b {
                    return Some(make_boolean_expr(true));
                }
                return Some(truncate_multi_value(rhs));
            }
            if is_nil(lhs) {
                return Some(truncate_multi_value(rhs));
            }
        }
        _ => {}
    }

    // Empty-string concat elision is subsumed by the decoded concat fold
    // above. (The old special-case arms also forgot to check the operator,
    // folding `"" < x` to `x`.)
    None
}

fn try_fold_unary(op: &Token, expr: &Expression, int_subtype: bool) -> Option<Expression> {
    match op.kind {
        TokenKind::Minus => {
            if let Some(n) = extract_lua_number(expr, int_subtype) {
                let negated = match n {
                    LuaNumber::Int(value) => LuaNumber::Int(value.wrapping_neg()),
                    LuaNumber::Float(value) => LuaNumber::Float(-value),
                };
                return make_lua_number_expr(negated, int_subtype);
            }
            // -(-x) -> x only when x is a number literal (metamethod/coercion safe)
            if let Expression::UnaryOp(inner) = expr
                && matches!(inner.op.kind, TokenKind::Minus)
                && matches!(inner.operand, Expression::Number(_))
            {
                return Some(inner.operand.clone());
            }
        }
        TokenKind::Not => {
            if let Some(b) = extract_boolean(expr) {
                return Some(make_boolean_expr(!b));
            }
            if is_nil(expr) {
                return Some(make_boolean_expr(true));
            }
            // Numbers and strings are always truthy in Lua
            if matches!(expr, Expression::Number(_) | Expression::StringLiteral(_)) {
                return Some(make_boolean_expr(false));
            }
        }
        TokenKind::Hash => {}
        _ => {}
    }
    None
}

/// Emit a folded number, or None when no literal can represent the value
/// exactly for the target's number model.
fn make_lua_number_expr(value: LuaNumber, int_subtype: bool) -> Option<Expression> {
    match value {
        LuaNumber::Int(int_value) => {
            // `-9223372036854775808` parses as unary minus on a literal
            // that overflows into a FLOAT in 5.3+ - the one integer with
            // no literal spelling. Refuse to fold it.
            if int_value == i64::MIN {
                return None;
            }
            if int_value < 0 {
                return Some(Expression::UnaryOp(Box::new(UnaryOp {
                    span: sp(),
                    op: Token::new(TokenKind::Minus, sp()),
                    operand: Expression::Number(Token::new(
                        TokenKind::Number(
                            itoa::Buffer::new().format(int_value.unsigned_abs()).into(),
                        ),
                        sp(),
                    )),
                })));
            }
            Some(Expression::Number(Token::new(
                TokenKind::Number(itoa::Buffer::new().format(int_value).into()),
                sp(),
            )))
        }
        LuaNumber::Float(float_value) => {
            if !float_value.is_finite() {
                return None;
            }
            // Preserve negative zero - 1/(-0) == -inf vs 1/0 == inf
            if float_value == 0.0 && float_value.is_sign_negative() {
                return Some(Expression::UnaryOp(Box::new(UnaryOp {
                    span: sp(),
                    op: Token::new(TokenKind::Minus, sp()),
                    operand: Expression::Number(Token::new(
                        TokenKind::Number(if int_subtype { "0.0" } else { "0" }.into()),
                        sp(),
                    )),
                })));
            }
            let magnitude = float_value.abs();
            // Rust's shortest-roundtrip Display; when the subtype is
            // observable the literal must SPELL float (`4.0`, not `4`).
            let mut text = format!("{magnitude}");
            if int_subtype && !text.contains('.') && !text.contains('e') && !text.contains('E') {
                text.push_str(".0");
            }
            let number = Expression::Number(Token::new(TokenKind::Number(text.into()), sp()));
            if float_value < 0.0 {
                Some(Expression::UnaryOp(Box::new(UnaryOp {
                    span: sp(),
                    op: Token::new(TokenKind::Minus, sp()),
                    operand: number,
                })))
            } else {
                Some(number)
            }
        }
    }
}

fn make_boolean_expr(value: bool) -> Expression {
    if value {
        Expression::True(Token::new(TokenKind::True, sp()))
    } else {
        Expression::False(Token::new(TokenKind::False, sp()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = fold(result.block, luck_token::LuaVersion::Lua54);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn test_fold_addition() {
        let result = apply("local x = 1 + 2\n");
        assert!(
            result.contains("3"),
            "Expected folded result, got: {result}"
        );
        assert!(
            !result.contains("+"),
            "Operator should be gone, got: {result}"
        );
    }

    #[test]
    fn test_fold_multiplication() {
        let result = apply("local x = 10 * 5\n");
        assert!(
            result.contains("50"),
            "Expected folded result, got: {result}"
        );
    }

    #[test]
    fn test_fold_string_concat() {
        let result = apply("local x = \"hello\" .. \" world\"\n");
        assert!(
            result.contains("\"hello world\""),
            "Expected folded string, got: {result}"
        );
    }

    #[test]
    fn test_fold_not_true() {
        let result = apply("local x = not true\n");
        assert!(result.contains("false"), "Expected `false`, got: {result}");
    }

    #[test]
    fn test_fold_not_false() {
        let result = apply("local x = not false\n");
        assert!(result.contains("true"), "Expected `true`, got: {result}");
    }

    #[test]
    fn test_fold_not_number() {
        let result = apply("local x = not 5\n");
        assert!(
            result.contains("false"),
            "not <number> should be false, got: {result}"
        );
    }

    #[test]
    fn test_fold_not_string() {
        let result = apply("local x = not \"hello\"\n");
        assert!(
            result.contains("false"),
            "not <string> should be false, got: {result}"
        );
    }

    #[test]
    fn test_hash_string_not_folded() {
        // #"str" must NOT be folded - escape sequences make raw length unreliable
        let result = apply("local x = #\"hello\"\n");
        assert!(
            result.contains("#"),
            "Should preserve #string, got: {result}"
        );
    }

    #[test]
    fn test_no_fold_division_by_zero() {
        let result = apply("local x = 1 / 0\n");
        assert!(
            result.contains("/"),
            "Should not fold division by zero: {result}"
        );
    }

    #[test]
    fn test_fold_and_true_lhs() {
        let result = apply("local x = true and 42\n");
        assert!(result.contains("42"), "Expected 42, got: {result}");
    }

    #[test]
    fn test_fold_and_false_lhs() {
        let result = apply("local x = false and 42\n");
        assert!(result.contains("false"), "Expected false, got: {result}");
    }

    #[test]
    fn test_fold_or_true_lhs() {
        let result = apply("local x = true or 42\n");
        assert!(result.contains("true"), "Expected true, got: {result}");
    }

    #[test]
    fn test_fold_or_false_lhs() {
        let result = apply("local x = false or 42\n");
        assert!(result.contains("42"), "Expected 42, got: {result}");
    }

    #[test]
    fn test_identity_add_zero_preserves_metamethods() {
        // a + 0 must NOT be folded - the + operator may invoke __add metamethod
        let result = apply("return a + 0\n");
        assert!(
            result.contains("+"),
            "Should preserve a + 0 (metamethod safety), got: {result}"
        );
    }

    #[test]
    fn test_comparison_negation_preserves_metamethods() {
        // not (a < b) must NOT become a >= b - different metamethods (__lt vs __le)
        let result = apply("return not (a < b)\n");
        assert!(
            !result.contains(">="),
            "Should not invert comparison (metamethod safety), got: {result}"
        );
    }

    #[test]
    fn test_fold_nil_and() {
        let result = apply("local x = nil and 42\n");
        assert!(result.contains("nil"), "Expected nil, got: {result}");
    }

    #[test]
    fn test_fold_nil_or() {
        let result = apply("local x = nil or 42\n");
        assert!(result.contains("42"), "Expected 42, got: {result}");
    }

    #[test]
    fn fold_negative_modulo() {
        // Lua: -5 % 3 = 1 (not -2 like Rust's truncated modulo)
        let result = apply("local x = -5 % 3\n");
        assert!(
            result.contains("local x=1"),
            "Expected 1 (Lua floored modulo), got: {result}"
        );
    }

    #[test]
    fn fold_modulo_negative_divisor() {
        // Lua: 5 % -3 = -1 (not 2 like Rust)
        let result = apply("local x = 5 % -3\n");
        assert!(
            result.contains("local x=-1"),
            "Expected -1 (Lua floored modulo), got: {result}"
        );
    }

    #[test]
    fn negative_zero_preserved() {
        let source = "local x = 0.0 * -1\nreturn 1/x";
        let result = apply(source);
        let parsed = luck_parser::parse(&result, luck_token::LuaVersion::Lua54);
        assert!(
            parsed.errors.is_empty(),
            "minified output should re-parse: {result}"
        );
        // Must not turn -0 into 0
        assert!(
            !result.contains("local x=0")
                || result.contains("local x=-0")
                || result.contains("local x=-(0)"),
            "Negative zero must be preserved, got: {result}"
        );
    }

    #[test]
    fn fold_modulo_by_zero() {
        // Should NOT fold (would be nan)
        let result = apply("local x = 5 % 0\n");
        assert!(
            result.contains("%"),
            "Should not fold modulo by zero, got: {result}"
        );
    }
}
