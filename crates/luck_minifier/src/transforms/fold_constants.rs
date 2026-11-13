use alloc::borrow::Cow;

use luck_ast::{Block, Expression, Literal, Transform, UnaryOp, unwrap_parentheses};
use luck_token::{
    BinOp, CompactString, LuaNumber, LuaVersion, NumberSubtypes, Span, UnOp, decode_string,
    encode_string, parse_lua_number,
};

use crate::expr::{extract_boolean, int_to_float, is_nil, truncate_multi_value};

pub(crate) fn fold(block: Block, version: LuaVersion) -> Block {
    ConstantFolder { version }.transform_block(block)
}

struct ConstantFolder {
    version: LuaVersion,
}

impl ConstantFolder {
    // Out of line, so an expression nested below holds only the walk's frame,
    // not these temporaries.
    fn fold_expr(&self, walked: Expression) -> Expression {
        match walked {
            Expression::BinaryOp(mut binary_op) => {
                match fold_binary(&binary_op.lhs, binary_op.op, &binary_op.rhs, self.version) {
                    Some(BinaryFold::Literal(literal)) => literal,
                    Some(BinaryFold::Left) => binary_op.lhs.take(),
                    Some(BinaryFold::Right) => truncate_multi_value(binary_op.rhs.take()),
                    None => Expression::BinaryOp(binary_op),
                }
            }
            Expression::UnaryOp(unop) => {
                fold_unary(unop.op, &unop.operand, NumberSubtypes::from(self.version))
                    .unwrap_or(Expression::UnaryOp(unop))
            }
            other @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_)) => other,
        }
    }
}

impl Transform for ConstantFolder {
    fn transform_expr(&mut self, expr: Expression) -> Expression {
        let walked = self.walk_expr(expr);
        self.fold_expr(walked)
    }
}

enum BinaryFold {
    Literal(Expression),
    /// The left operand, as written.
    Left,
    /// The right operand, truncated to one value.
    Right,
}

/// Integers wrap, so `i64::MIN` negates to itself.
#[expect(clippy::float_arithmetic, reason = "this is Lua unary minus, evaluated at compile time")]
fn negate_number(number: LuaNumber) -> LuaNumber {
    match number {
        LuaNumber::Int(value) => LuaNumber::Int(value.wrapping_neg()),
        LuaNumber::Float(value) => LuaNumber::Float(-value),
    }
}

/// Reads a number literal, possibly negated or parenthesized, with its
/// subtype. Under [`NumberSubtypes::Unified`] (5.1, 5.2, Luau) every number
/// is a float.
fn extract_lua_number(expr: &Expression, subtypes: NumberSubtypes) -> Option<LuaNumber> {
    match expr {
        Expression::Number(literal) => parse_lua_number(&literal.text, subtypes),
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Neg) => {
            extract_lua_number(&unop.operand, subtypes).map(negate_number)
        }
        Expression::Parenthesized(paren) => {
            extract_lua_number(unwrap_parentheses(&paren.expr), subtypes)
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => None,
    }
}

/// Whether the integer is within 2^53, where every integer converts to f64
/// exactly. Mixed int/float folds stop beyond it, since the conversion could
/// lose precision.
///
/// `i64::MIN` has no positive counterpart, so `wrapping_abs` leaves it negative
/// and it passes. That's fine: -2^63 is a power of two, exact in f64.
const fn int_fits_f64(value: i64) -> bool {
    value.wrapping_abs() <= (1i64 << 53)
}

fn fold_numeric(
    lhs: LuaNumber,
    op: BinOp,
    rhs: LuaNumber,
    version: LuaVersion,
) -> Option<LuaNumber> {
    use LuaNumber::{Float, Int};
    match (lhs, rhs) {
        (Int(left), Int(right)) => match op {
            // Integer ops wrap, matching Lua 5.3+ exactly.
            BinOp::Add => Some(Int(left.wrapping_add(right))),
            BinOp::Sub => Some(Int(left.wrapping_sub(right))),
            BinOp::Mul => Some(Int(left.wrapping_mul(right))),
            // `/` and `^` always produce floats.
            BinOp::Div | BinOp::Pow => {
                fold_float(int_to_float(left), op, int_to_float(right), version)
            }
            // Integer % 0 and // 0 raise at runtime, so never fold them.
            BinOp::Mod => floored_mod(left, right).map(Int),
            // Lua 5.3+, Luau
            BinOp::FloorDiv => floored_div(left, right).map(Int),
            BinOp::Concat
            | BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::And
            | BinOp::Or
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr => None,
        },
        (Float(left), Float(right)) => fold_float(left, op, right, version),
        (Int(left), Float(right)) => {
            if !int_fits_f64(left) {
                return None;
            }
            fold_float(int_to_float(left), op, right, version)
        }
        (Float(left), Int(right)) => {
            // 5.4+ compiles `x - 0` as an immediate add, so -0.0 keeps or loses
            // its sign depending on the target. Leaving it unfolded is right
            // for every target.
            let is_negative_zero_minus_zero =
                op == BinOp::Sub && right == 0 && left == 0.0f64 && left.is_sign_negative();
            if !int_fits_f64(right) || is_negative_zero_minus_zero {
                return None;
            }
            fold_float(left, op, int_to_float(right), version)
        }
    }
}

/// `base ^ exponent` when both are integers and every partial product is
/// exact. Anything else goes through the target's C `pow`, whose rounding
/// isn't ours to reproduce.
#[expect(
    clippy::float_arithmetic,
    clippy::while_float,
    reason = "exact integer powers, computed at compile time"
)]
fn exact_power(base: f64, exponent: f64) -> Option<f64> {
    if base.fract() != 0.0f64
        || exponent.fract() != 0.0f64
        || !(0.0f64..=53.0f64).contains(&exponent)
    {
        return None;
    }
    let exact_limit = int_to_float(1i64 << 53);
    let mut power = 1.0f64;
    let mut remaining = exponent;
    while remaining >= 1.0f64 {
        power *= base;
        if power.abs() >= exact_limit {
            return None;
        }
        remaining -= 1.0f64;
    }
    Some(power)
}

fn floored_mod(lhs: i64, rhs: i64) -> Option<i64> {
    // `x % -1` is 0 for every x. `checked_rem` refuses `i64::MIN % -1`
    // because the quotient overflows.
    if rhs == -1 {
        return Some(0);
    }
    let remainder = lhs.checked_rem(rhs)?;
    if remainder != 0 && (remainder < 0) != (rhs < 0) {
        Some(remainder.wrapping_add(rhs))
    } else {
        Some(remainder)
    }
}

fn floored_div(lhs: i64, rhs: i64) -> Option<i64> {
    // Lua wraps `i64::MIN // -1` to i64::MIN, which has no decimal literal
    // (see make_lua_number_expr), so an overflowing division isn't folded.
    let quotient = lhs.checked_div(rhs)?;
    let remainder = lhs.checked_rem(rhs)?;
    if remainder != 0 && (remainder < 0) != (rhs < 0) {
        quotient.checked_sub(1)
    } else {
        Some(quotient)
    }
}

#[expect(
    clippy::suboptimal_flops,
    reason = "Lua's floor formula rounds twice, and mul_add would round once"
)]
#[expect(
    clippy::float_arithmetic,
    clippy::modulo_arithmetic,
    reason = "Lua float arithmetic at compile time; 5.3+ % is C fmod"
)]
fn fold_float(lhs: f64, op: BinOp, rhs: f64, version: LuaVersion) -> Option<LuaNumber> {
    let value = match op {
        BinOp::Add => lhs + rhs,
        BinOp::Sub => lhs - rhs,
        BinOp::Mul => lhs * rhs,
        BinOp::Div if rhs != 0.0f64 => lhs / rhs,
        BinOp::Mod if rhs != 0.0f64 => {
            if version.has_fmod_float_modulo() {
                // 5.3+ luai_nummod: fmod plus a sign fix. The floor
                // formula is wrong at large magnitudes (1e16 % 3) and
                // for the sign of zero results (-6.0 % 3.0 is -0.0).
                let remainder = lhs % rhs;
                let should_add_divisor = if version.has_float_modulo_sign_compare() {
                    // Lua 5.4+
                    if remainder > 0.0f64 {
                        rhs < 0.0f64
                    } else {
                        remainder < 0.0f64 && rhs > 0.0f64
                    }
                } else {
                    // Lua 5.3
                    remainder * rhs < 0.0f64
                };
                if should_add_divisor { remainder + rhs } else { remainder }
            } else {
                // 5.1, 5.2, and Luau: a - floor(a/b)*b, exactly.
                lhs - (lhs / rhs).floor() * rhs
            }
        }
        BinOp::Pow => return exact_power(lhs, rhs).map(LuaNumber::Float),
        // Lua 5.3+, Luau
        BinOp::FloorDiv if rhs != 0.0f64 => (lhs / rhs).floor(),
        BinOp::Div
        | BinOp::Mod
        | BinOp::FloorDiv
        | BinOp::Concat
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => return None,
    };
    Some(LuaNumber::Float(value))
}

fn compare_numbers(lhs: LuaNumber, op: BinOp, rhs: LuaNumber) -> Option<bool> {
    use core::cmp::Ordering;

    use LuaNumber::{Float, Int};
    let ordering = match (lhs, rhs) {
        (Int(left), Int(right)) => left.cmp(&right),
        (Float(left), Float(right)) => left.partial_cmp(&right)?,
        // Lua compares mixed int/float exactly, so go through f64 only when
        // the int converts exactly.
        (Int(left), Float(right)) => {
            if !int_fits_f64(left) {
                return None;
            }
            int_to_float(left).partial_cmp(&right)?
        }
        (Float(left), Int(right)) => {
            if !int_fits_f64(right) {
                return None;
            }
            left.partial_cmp(&int_to_float(right))?
        }
    };
    Some(match op {
        BinOp::Lt => ordering == Ordering::Less,
        BinOp::Gt => ordering == Ordering::Greater,
        BinOp::Le => ordering != Ordering::Greater,
        BinOp::Ge => ordering != Ordering::Less,
        BinOp::Eq => ordering == Ordering::Equal,
        BinOp::Ne => ordering != Ordering::Equal,
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::Concat
        | BinOp::And
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => return None,
    })
}

fn extract_string_bytes(expr: &Expression, version: LuaVersion) -> Option<Cow<'_, [u8]>> {
    match expr {
        Expression::StringLiteral(literal) => decode_string(&literal.text, version),
        Expression::Parenthesized(paren) => {
            extract_string_bytes(unwrap_parentheses(&paren.expr), version)
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => None,
    }
}

/// Strings compare by decoded bytes, since `"\65"` and `"A"` are the same string.
fn fold_equality(lhs: &Expression, rhs: &Expression, version: LuaVersion) -> Option<bool> {
    if let (Some(left), Some(right)) =
        (extract_string_bytes(lhs, version), extract_string_bytes(rhs, version))
    {
        return Some(left == right);
    }
    Some(extract_boolean(lhs)? == extract_boolean(rhs)?)
}

/// Joins decoded bytes, since gluing raw text can merge escapes:
/// `"\1" .. "2"` isn't `"\12"`.
fn fold_concat(lhs: &Expression, rhs: &Expression, version: LuaVersion) -> Option<Expression> {
    let mut joined = extract_string_bytes(lhs, version)?.into_owned();
    joined.extend_from_slice(&extract_string_bytes(rhs, version)?);
    Some(Expression::StringLiteral(Literal { text: encode_string(&joined), span: Span::default() }))
}

fn fold_binary(
    lhs: &Expression,
    op: BinOp,
    rhs: &Expression,
    version: LuaVersion,
) -> Option<BinaryFold> {
    let subtypes = NumberSubtypes::from(version);
    if let (Some(left), Some(right)) =
        (extract_lua_number(lhs, subtypes), extract_lua_number(rhs, subtypes))
    {
        if let Some(value) = fold_numeric(left, op, right, version)
            && let Some(folded) = make_lua_number_expr(value, subtypes)
        {
            return Some(BinaryFold::Literal(folded));
        }
        if let Some(value) = compare_numbers(left, op, right) {
            return Some(BinaryFold::Literal(make_boolean_expr(value)));
        }
    }

    match op {
        BinOp::Eq => fold_equality(lhs, rhs, version)
            .map(|is_equal| BinaryFold::Literal(make_boolean_expr(is_equal))),
        BinOp::Ne => fold_equality(lhs, rhs, version)
            .map(|is_equal| BinaryFold::Literal(make_boolean_expr(!is_equal))),
        BinOp::Concat => fold_concat(lhs, rhs, version).map(BinaryFold::Literal),
        BinOp::And => match extract_boolean(lhs) {
            Some(true) => Some(BinaryFold::Right),
            Some(false) => Some(BinaryFold::Literal(make_boolean_expr(false))),
            None => is_nil(lhs).then_some(BinaryFold::Left),
        },
        BinOp::Or => match extract_boolean(lhs) {
            Some(true) => Some(BinaryFold::Literal(make_boolean_expr(true))),
            Some(false) => Some(BinaryFold::Right),
            None => is_nil(lhs).then_some(BinaryFold::Right),
        },
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => None,
    }
}

fn fold_unary(op: UnOp, expr: &Expression, subtypes: NumberSubtypes) -> Option<Expression> {
    match op {
        UnOp::Neg => {
            if let Some(number) = extract_lua_number(expr, subtypes) {
                return make_lua_number_expr(negate_number(number), subtypes);
            }
            // -(-x) folds to x only for a number literal, where neither a
            // metamethod nor a string coercion can intervene.
            if let Expression::UnaryOp(inner) = expr
                && inner.op == UnOp::Neg
                && matches!(inner.operand, Expression::Number(_))
            {
                return Some(inner.operand.clone());
            }
        }
        UnOp::Not => {
            if let Some(value) = extract_boolean(expr) {
                return Some(make_boolean_expr(!value));
            }
            if is_nil(expr) {
                return Some(make_boolean_expr(true));
            }
            // Numbers and strings are always truthy in Lua.
            if matches!(expr, Expression::Number(_) | Expression::StringLiteral(_)) {
                return Some(make_boolean_expr(false));
            }
        }
        UnOp::Len | UnOp::BitNot => {}
    }
    None
}

/// Builds the literal for a folded number, or `None` when no literal spells
/// the value exactly under the target's number model.
fn make_lua_number_expr(value: LuaNumber, subtypes: NumberSubtypes) -> Option<Expression> {
    let has_int_subtype = subtypes == NumberSubtypes::IntFloat;
    match value {
        LuaNumber::Int(int_value) => {
            // `-9223372036854775808` is unary minus on a literal that
            // overflows to a float in 5.3+, so i64::MIN has no decimal
            // spelling.
            if int_value == i64::MIN {
                return None;
            }
            let magnitude =
                number_literal(itoa::Buffer::new().format(int_value.unsigned_abs()).into());
            Some(if int_value < 0 { negated(magnitude) } else { magnitude })
        }
        LuaNumber::Float(float_value) => {
            if !float_value.is_finite() {
                return None;
            }
            // Preserve negative zero: 1/(-0) is -inf where 1/0 is inf.
            if float_value == 0.0 && float_value.is_sign_negative() {
                let zero = if has_int_subtype { "0.0" } else { "0" };
                return Some(negated(number_literal(zero.into())));
            }
            let magnitude = float_value.abs();
            // Display gives the shortest round-trip digits. On 5.3+ the
            // literal must still spell a float (`4.0`, not `4`).
            let mut text = format!("{magnitude}");
            if has_int_subtype && !text.contains('.') && !text.contains('e') && !text.contains('E')
            {
                text.push_str(".0");
            }
            let number = number_literal(text.into());
            Some(if float_value < 0.0 { negated(number) } else { number })
        }
    }
}

fn number_literal(text: CompactString) -> Expression {
    Expression::Number(Literal { text, span: Span::default() })
}

fn negated(operand: Expression) -> Expression {
    Expression::UnaryOp(Box::new(UnaryOp { span: Span::default(), op: UnOp::Neg, operand }))
}

fn make_boolean_expr(value: bool) -> Expression {
    if value { Expression::True(Span::default()) } else { Expression::False(Span::default()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        run_pass(source, version, |block| fold(block, version))
    }

    #[test]
    fn folds_addition() {
        let result = apply("local x = 1 + 2\n");
        assert!(result.contains('3'), "Expected folded result, got: {result}");
        assert!(!result.contains('+'), "Operator should be gone, got: {result}");
    }

    #[test]
    fn folds_multiplication() {
        let result = apply("local x = 10 * 5\n");
        assert!(result.contains("50"), "Expected folded result, got: {result}");
    }

    #[test]
    fn folds_string_concat() {
        let result = apply("local x = \"hello\" .. \" world\"\n");
        assert!(result.contains("\"hello world\""), "Expected folded string, got: {result}");
    }

    #[test]
    fn folds_not_true() {
        let result = apply("local x = not true\n");
        assert!(result.contains("false"), "Expected `false`, got: {result}");
    }

    #[test]
    fn folds_not_false() {
        let result = apply("local x = not false\n");
        assert!(result.contains("true"), "Expected `true`, got: {result}");
    }

    #[test]
    fn folds_not_of_a_number_to_false() {
        let result = apply("local x = not 5\n");
        assert!(result.contains("false"), "not <number> should be false, got: {result}");
    }

    #[test]
    fn folds_not_of_a_string_to_false() {
        let result = apply("local x = not \"hello\"\n");
        assert!(result.contains("false"), "not <string> should be false, got: {result}");
    }

    #[test]
    fn keeps_length_of_a_string_literal() {
        // #"str" must not be folded: escape sequences make the raw length
        // unreliable.
        let result = apply("local x = #\"hello\"\n");
        assert!(result.contains('#'), "Should preserve #string, got: {result}");
    }

    #[test]
    fn keeps_division_by_zero() {
        let result = apply("local x = 1 / 0\n");
        assert!(result.contains('/'), "Should not fold division by zero: {result}");
    }

    #[test]
    fn folds_and_with_a_true_left_operand() {
        let result = apply("local x = true and 42\n");
        assert!(result.contains("42"), "Expected 42, got: {result}");
    }

    #[test]
    fn folds_and_with_a_false_left_operand() {
        let result = apply("local x = false and 42\n");
        assert!(result.contains("false"), "Expected false, got: {result}");
    }

    #[test]
    fn folds_or_with_a_true_left_operand() {
        let result = apply("local x = true or 42\n");
        assert!(result.contains("true"), "Expected true, got: {result}");
    }

    #[test]
    fn folds_or_with_a_false_left_operand() {
        let result = apply("local x = false or 42\n");
        assert!(result.contains("42"), "Expected 42, got: {result}");
    }

    #[test]
    fn keeps_adding_zero_to_a_variable() {
        // a + 0 must not be folded: `+` may invoke an __add metamethod.
        let result = apply("return a + 0\n");
        assert!(result.contains('+'), "Should preserve a + 0 (metamethod safety), got: {result}");
    }

    #[test]
    fn never_inverts_a_negated_comparison() {
        // not (a < b) must not become a >= b: the two call different
        // metamethods, __lt and __le.
        let result = apply("return not (a < b)\n");
        assert!(
            !result.contains(">="),
            "Should not invert comparison (metamethod safety), got: {result}"
        );
    }

    #[test]
    fn folds_and_with_a_nil_left_operand() {
        let result = apply("local x = nil and 42\n");
        assert!(result.contains("nil"), "Expected nil, got: {result}");
    }

    #[test]
    fn folds_or_with_a_nil_left_operand() {
        let result = apply("local x = nil or 42\n");
        assert!(result.contains("42"), "Expected 42, got: {result}");
    }

    #[test]
    fn folds_modulo_of_a_negative_dividend_with_floored_sign() {
        // Lua: -5 % 3 = 1 (not -2 like Rust's truncated modulo)
        let result = apply("local x = -5 % 3\n");
        assert!(result.contains("local x=1"), "Expected 1 (Lua floored modulo), got: {result}");
    }

    #[test]
    fn folds_modulo_by_a_negative_divisor_with_floored_sign() {
        // Lua: 5 % -3 = -1 (not 2 like Rust)
        let result = apply("local x = 5 % -3\n");
        assert!(result.contains("local x=-1"), "Expected -1 (Lua floored modulo), got: {result}");
    }

    #[test]
    fn keeps_negative_zero() {
        let source = "local x = 0.0 * -1\nreturn 1/x";
        let result = apply(source);
        assert_reparses(&result, LuaVersion::Lua54);
        // Must not turn -0 into 0.
        assert!(
            !result.contains("local x=0")
                || result.contains("local x=-0")
                || result.contains("local x=-(0)"),
            "Negative zero must be preserved, got: {result}"
        );
    }

    #[test]
    fn keeps_integer_modulo_by_zero() {
        // Integer modulo by zero raises at runtime.
        let result = apply("local x = 5 % 0\n");
        assert!(result.contains('%'), "Should not fold modulo by zero, got: {result}");
    }

    #[test]
    fn folds_int_min_mixed_with_a_float_only_where_exact() {
        // `0x8000000000000000` wraps to i64::MIN, whose f64 image is exact, so
        // it folds. Its neighbor has no exact image and must not.
        for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
            for (source, expected) in [
                ("return 0x8000000000000000 + 1.5\n", "return-9223372036854776000.0"),
                ("return 1.5 + 0x8000000000000000\n", "return-9223372036854776000.0"),
                ("return 0x8000000000000000 < 1.5\n", "return true"),
                ("return 0x8000000000000001 + 1.5\n", "return 0x8000000000000001+1.5"),
            ] {
                assert_eq!(apply_version(source, version), expected, "{version:?}");
            }
        }
    }

    #[test]
    fn folds_float_modulo_with_fmod_from_lua53() {
        // 5.3+ luai_nummod is fmod + sign fix: 1e16 % 3 == 1.0 exactly.
        // The floor formula gives 0.0 (precision loss at 1e16).
        let result = apply_version("return 1e16 % 3\n", LuaVersion::Lua54);
        assert!(result.contains('1'), "5.4 folds 1e16%3 to 1.0: {result}");
        assert!(!result.contains('%'), "must fold: {result}");
        // 5.1's floor formula really does produce 0.0, so mirror it.
        let lua51_result = apply_version("return 1e16 % 3\n", LuaVersion::Lua51);
        assert!(!lua51_result.contains('%'), "5.1 folds too: {lua51_result}");
        assert!(lua51_result.contains('0'), "5.1 floor formula gives 0: {lua51_result}");
    }

    #[test]
    fn folds_float_modulo_keeping_the_sign_of_zero_from_lua53() {
        // fmod(-6.0, 3.0) is -0.0 under 5.3+; the floor formula loses
        // the sign. Observable via 1/x and tostring.
        let result = apply_version("return -6.0 % 3.0\n", LuaVersion::Lua54);
        assert!(result.contains("-0"), "5.4 keeps the sign of zero: {result}");
        let lua51_result = apply_version("return -6.0 % 3.0\n", LuaVersion::Lua51);
        assert!(!lua51_result.contains("-0"), "5.1 floor formula gives +0: {lua51_result}");
    }

    #[test]
    fn subtracting_integer_zero_matches_the_lua54_immediate_add() {
        // 5.4+ compiles `x - 0` to OP_ADDI with -0, and -0.0 + 0.0 is +0.0.
        for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
            let result = apply_version("return -0.0 - 0\n", version);
            assert_ne!(result, "return-0.0", "{version:?} evaluates this to +0.0");
        }
    }

    #[test]
    fn folds_integer_powers_with_an_exact_result() {
        for (source, expected) in [
            ("return 2 ^ 10\n", "return 1024.0"),
            ("return (-2) ^ 3\n", "return-8.0"),
            ("return 7 ^ 0\n", "return 1.0"),
            ("return 2 ^ 53\n", "return 2^53"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_exponentiation_with_an_inexact_result() {
        let result = apply("return 0.3 ^ 3\n");
        assert!(
            result.contains('^'),
            "an inexact `^` depends on the target's C pow, so it must not fold: {result}"
        );
    }
}
