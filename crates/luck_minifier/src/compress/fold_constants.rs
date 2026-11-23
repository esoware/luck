use core::cmp::Ordering;

use luck_allocator::{Allocator, Box, ThinBytes};
use luck_ast::{Expression, GetSpan as _, NumberLiteral, OneOrMore};
use luck_token::{Associativity, BinOp, LuaNumber, LuaVersion, Span, UnOp};

use crate::expr::{boolean, extract_boolean, is_nil, truncate_multi_value};

/// Folds a run of binary operators or a unary operator over literals, and
/// says whether it did. Hands `drop` each operand it drops that isn't a
/// literal, which only `and` and `or` drop.
pub(super) fn fold<'ast>(
    allocator: &'ast Allocator,
    version: LuaVersion,
    expr: &mut Expression<'ast>,
    drop: &mut impl FnMut(&Expression<'ast>),
) -> bool {
    match expr {
        Expression::BinaryOp(run) => match run.precedence().1 {
            Associativity::Left => fold_left_run(allocator, version, expr, drop),
            Associativity::Right if run.links.first().op == BinOp::Concat => {
                fold_concat_run(allocator, expr)
            }
            Associativity::Right => fold_right_run(allocator, version, expr),
        },
        Expression::UnaryOp(_) => fold_unary(allocator, version, expr),
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

enum BinaryFold<'ast> {
    Literal(Expression<'ast>),
    /// The left operand, as written.
    Left,
    /// The right operand, truncated to one value.
    Right,
}

/// Folds the run's operators from the left while they fold, as the pairs it
/// groups into would.
///
/// An operator that doesn't fold leaves a run on its left, which no later one
/// folds with either.
fn fold_left_run<'ast>(
    allocator: &'ast Allocator,
    version: LuaVersion,
    expr: &mut Expression<'ast>,
    drop: &mut impl FnMut(&Expression<'ast>),
) -> bool {
    let Expression::BinaryOp(boxed) = expr else {
        return false;
    };
    let run = &mut **boxed;
    let mut folded = None;
    let mut folded_links = 0usize;
    for link in &mut *run.links {
        let left = folded.as_ref().unwrap_or(&run.first);
        match fold_binary(allocator, left, link.op, &link.operand, version) {
            Some(BinaryFold::Literal(literal)) => {
                if matches!(link.op, BinOp::And | BinOp::Or) {
                    drop(&link.operand);
                }
                folded = Some(literal);
            }
            Some(BinaryFold::Left) => {
                drop(&link.operand);
                if folded.is_none() {
                    folded = Some(run.first.take());
                }
            }
            Some(BinaryFold::Right) => {
                folded = Some(truncate_multi_value(allocator, link.operand.take()));
            }
            None => break,
        }
        folded_links = folded_links.saturating_add(1);
    }
    let Some(value) = folded else {
        return false;
    };
    match OneOrMore::from_list(run.links.split_off(folded_links)) {
        Some(rest) => {
            run.first = value;
            run.links = rest;
        }
        None => *expr = value,
    }
    true
}

/// Folds the run's `^` from the right for as long as they fold, as the
/// nested pairs it groups into would.
fn fold_right_run<'ast>(
    allocator: &'ast Allocator,
    version: LuaVersion,
    expr: &mut Expression<'ast>,
) -> bool {
    let Expression::BinaryOp(boxed) = expr else {
        return false;
    };
    let run = &mut **boxed;
    // The link whose operand `folded` replaces, along with every link after it.
    let mut pending = run.links.len().saturating_sub(1);
    let mut folded = None;
    while let Some(link) = run.links.get(pending) {
        let right = folded.as_ref().unwrap_or(&link.operand);
        let previous_link = pending.checked_sub(1);
        let left = previous_link
            .and_then(|i| run.links.get(i))
            .map_or(&run.first, |left_link| &left_link.operand);
        let Some(BinaryFold::Literal(literal)) =
            fold_binary(allocator, left, link.op, right, version)
        else {
            break;
        };
        let Some(previous) = previous_link else {
            *expr = literal;
            return true;
        };
        folded = Some(literal);
        pending = previous;
    }
    let Some(value) = folded else {
        return false;
    };
    run.links.truncate(pending.saturating_add(1));
    run.links.last_mut().operand = value;
    true
}

/// Integers wrap, so `i64::MIN` negates to itself.
#[expect(clippy::float_arithmetic, reason = "this is Lua unary minus, evaluated at compile time")]
fn negate_number(number: LuaNumber) -> LuaNumber {
    match number {
        LuaNumber::Int(value) => LuaNumber::Int(value.wrapping_neg()),
        LuaNumber::Float(value) => LuaNumber::Float(-value),
    }
}

/// The value of a number literal, possibly negated.
fn extract_lua_number(expr: &Expression<'_>) -> Option<LuaNumber> {
    match expr {
        Expression::Number(literal) => Some(literal.value),
        Expression::UnaryOp(unop) if unop.op == UnOp::Neg => {
            extract_lua_number(&unop.operand).map(negate_number)
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
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
                fold_float(Int(left).to_f64(), op, Int(right).to_f64(), version)
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
            fold_float(Int(left).to_f64(), op, right, version)
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
            fold_float(left, op, Int(right).to_f64(), version)
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
    let exact_limit = LuaNumber::Int(1i64 << 53).to_f64();
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
            Int(left).to_f64().partial_cmp(&right)?
        }
        (Float(left), Int(right)) => {
            if !int_fits_f64(right) {
                return None;
            }
            left.partial_cmp(&Int(right).to_f64())?
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

/// The value of a string literal.
fn extract_string<'ast>(expr: &Expression<'ast>) -> Option<ThinBytes<'ast>> {
    if let Expression::StringLiteral(literal) = expr { Some(literal.value) } else { None }
}

/// Strings compare by value, so `"\65"` and `"A"` are the same string.
fn fold_equality(lhs: &Expression<'_>, rhs: &Expression<'_>) -> Option<bool> {
    if let Some(left) = extract_string(lhs)
        && let Some(right) = extract_string(rhs)
    {
        return Some(left == right);
    }
    Some(extract_boolean(lhs)? == extract_boolean(rhs)?)
}

/// Joins the string literals a run of `..` ends with into one.
///
/// `..` groups from the right, so those are the pairs it folds first, and
/// they join in one copy however many there are. A string never joins a
/// number, whose `tostring` is the target's.
fn fold_concat_run<'ast>(allocator: &'ast Allocator, expr: &mut Expression<'ast>) -> bool {
    let Expression::BinaryOp(boxed) = expr else {
        return false;
    };
    let run = &mut **boxed;
    let joined_links =
        run.links.iter().rev().take_while(|link| extract_string(&link.operand).is_some()).count();
    let first_link = run.links.len().saturating_sub(joined_links);
    let is_whole_run = first_link == 0 && extract_string(&run.first).is_some();
    if joined_links < 2 && !(is_whole_run && joined_links == 1) {
        return false;
    }
    let tail = run.links.get(first_link..).unwrap_or_default().iter().map(|link| &link.operand);
    let operands = is_whole_run.then_some(&run.first).into_iter().chain(tail);
    let mut joined = Vec::new();
    let mut span = None;
    for operand in operands {
        if let Some(string) = extract_string(operand) {
            joined.extend_from_slice(&string);
        }
        span = Some(span.map_or_else(|| operand.span(), |start: Span| start.merge(operand.span())));
    }
    let value = ThinBytes::new(allocator.alloc_slice_copy(&joined));
    let literal = Expression::new_string_literal(value, None, span.unwrap_or_default(), allocator);
    if is_whole_run {
        *expr = literal;
    } else {
        run.links.truncate(first_link.saturating_add(1));
        run.links.last_mut().operand = literal;
    }
    true
}

fn fold_binary<'ast>(
    allocator: &'ast Allocator,
    lhs: &Expression<'_>,
    op: BinOp,
    rhs: &Expression<'_>,
    version: LuaVersion,
) -> Option<BinaryFold<'ast>> {
    let span = lhs.span().merge(rhs.span());
    if let Some(left) = extract_lua_number(lhs)
        && let Some(right) = extract_lua_number(rhs)
    {
        if let Some(value) = fold_numeric(left, op, right, version)
            && let Some(folded) = make_lua_number_expr(allocator, value, span)
        {
            return Some(BinaryFold::Literal(folded));
        }
        if let Some(value) = compare_numbers(left, op, right) {
            return Some(BinaryFold::Literal(boolean(value, span)));
        }
    }

    match op {
        BinOp::Eq => {
            fold_equality(lhs, rhs).map(|is_equal| BinaryFold::Literal(boolean(is_equal, span)))
        }
        BinOp::Ne => {
            fold_equality(lhs, rhs).map(|is_equal| BinaryFold::Literal(boolean(!is_equal, span)))
        }
        BinOp::And => match extract_boolean(lhs) {
            Some(true) => Some(BinaryFold::Right),
            Some(false) => Some(BinaryFold::Literal(boolean(false, span))),
            None => is_nil(lhs).then_some(BinaryFold::Left),
        },
        BinOp::Or => match extract_boolean(lhs) {
            Some(true) => Some(BinaryFold::Literal(boolean(true, span))),
            Some(false) => Some(BinaryFold::Right),
            None => is_nil(lhs).then_some(BinaryFold::Right),
        },
        // A run of `..` folds as one, in `fold_concat_run`.
        BinOp::Concat
        | BinOp::Add
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

fn fold_unary<'ast>(
    allocator: &'ast Allocator,
    version: LuaVersion,
    expr: &mut Expression<'ast>,
) -> bool {
    let Expression::UnaryOp(unop) = expr else {
        return false;
    };
    let span = unop.span;
    let folded = match unop.op {
        UnOp::Neg => {
            // A negated numeral is how a negative number is spelled, so only
            // the numeral's own spelling can get shorter.
            if matches!(unop.operand, Expression::Number(_)) {
                return false;
            }
            if let Some(number) = extract_lua_number(&unop.operand) {
                make_lua_number_expr(allocator, negate_number(number), span)
            } else if let Expression::UnaryOp(inner) = &unop.operand
                && inner.op == UnOp::Neg
                && let Expression::Number(literal) = &inner.operand
            {
                // -(-x) folds to x only for a number literal, where neither a
                // metamethod nor a string coercion can intervene.
                Some(Expression::Number(Box::new_in(**literal, allocator)))
            } else {
                None
            }
        }
        UnOp::Not => {
            if let Some(value) = extract_boolean(&unop.operand) {
                Some(boolean(!value, span))
            } else if is_nil(&unop.operand) {
                Some(boolean(true, span))
            } else if matches!(unop.operand, Expression::Number(_) | Expression::StringLiteral(_)) {
                // Numbers and strings are always truthy in Lua.
                Some(boolean(false, span))
            } else {
                None
            }
        }
        // `#` on a string is its byte count, which no metamethod changes.
        UnOp::Len => extract_string(&unop.operand).and_then(|string| {
            let length = i64::try_from(string.len()).ok()?;
            let value = if version.has_integer_subtype() {
                LuaNumber::Int(length)
            } else {
                LuaNumber::Float(LuaNumber::Int(length).to_f64())
            };
            make_lua_number_expr(allocator, value, span)
        }),
        UnOp::BitNot => None,
    };
    let Some(value) = folded else {
        return false;
    };
    *expr = value;
    true
}

/// The expression for a folded number: a literal, under a `-` when the
/// value is negative, or `None` for NaN, which no literal holds.
///
/// `i64::MIN` has no magnitude to negate, so it's a literal, which prints as
/// the hex that wraps to it on Lua 5.3+. A negative zero keeps its sign
/// under the `-`, since `1/-0` is `-inf`.
fn make_lua_number_expr(
    allocator: &Allocator,
    value: LuaNumber,
    span: Span,
) -> Option<Expression<'_>> {
    let literal = |magnitude| {
        Expression::Number(Box::new_in(NumberLiteral::new(magnitude, None, span, true), allocator))
    };
    let negated = |operand| Expression::new_unary_op(span, UnOp::Neg, operand, allocator);
    match value {
        LuaNumber::Int(integer) => Some(match integer.checked_neg() {
            Some(magnitude) if integer < 0 => negated(literal(LuaNumber::Int(magnitude))),
            Some(_) | None => literal(value),
        }),
        LuaNumber::Float(float) if float.is_nan() => None,
        LuaNumber::Float(float) if float.is_sign_negative() => {
            Some(negated(literal(LuaNumber::Float(float.abs()))))
        }
        LuaNumber::Float(_) => Some(literal(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const FOLDING: TransformConfig = TransformConfig { fold_constants: true, ..NO_PASSES };

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        minify_with(source, version, &FOLDING)
    }

    #[test]
    fn folds_arithmetic_and_concatenation() {
        for (source, expected) in [
            ("local x = 1 + 2", "local x=3"),
            ("local x = 10 * 5", "local x=50"),
            ("local x = \"hello\" .. \" world\"", "local x=\"hello world\""),
            ("local x = 2 + 3 * 4 - 1", "local x=13"),
            ("local x = \"a\" .. \"b\" .. \"c\"", "local x=\"abc\""),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_not_of_a_literal() {
        for (source, expected) in [
            ("local x = not true", "local x=false"),
            ("local x = not false", "local x=true"),
            ("local x = not nil", "local x=true"),
            ("local x = not 5", "local x=false"),
            ("local x = not \"hello\"", "local x=false"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_and_and_or_with_a_literal_left_operand() {
        for (source, expected) in [
            ("local x = true and 42", "local x=42"),
            ("local x = false and 42", "local x=false"),
            ("local x = true or 42", "local x=true"),
            ("local x = false or 42", "local x=42"),
            ("local x = nil and 42", "local x=nil"),
            ("local x = nil or 42", "local x=42"),
            ("return true and f()", "return(f())"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_what_could_raise_or_call_a_metamethod() {
        for source in [
            "local x=1/0",
            "local x=5%0",
            // `+` may invoke an __add metamethod.
            "return a+0",
            "return#t",
        ] {
            assert_eq!(apply(source), source);
        }
    }

    #[test]
    fn folds_the_length_of_a_string_by_its_value() {
        for (source, expected, version) in [
            ("return #\"hello\"", "return 5", LuaVersion::Lua54),
            ("return #'\\65\\u{e9}' + 1", "return 4", LuaVersion::Lua54),
            ("return #[[\nab]]", "return 2", LuaVersion::Lua51),
            ("return #(\"ab\") * 1.5", "return 3", LuaVersion::Luau),
        ] {
            assert_eq!(apply_version(source, version), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_an_overflow_to_infinity() {
        assert_eq!(apply("local x = 1e308 * 10"), "local x=1e999");
        assert_eq!(apply("local x = -1e308 * 10"), "local x=-1e999");
        assert_eq!(apply("local x = 1e308 * 10 - 1e308 * 10"), "local x=1e999-1e999");
    }

    #[test]
    fn never_inverts_a_negated_comparison() {
        // not (a < b) must not become a >= b: the two call different
        // metamethods, __lt and __le.
        let result = apply("return not (a < b)\n");
        assert!(!result.contains(">="), "{result}");
    }

    #[test]
    fn folds_modulo_with_the_floored_sign() {
        // Lua: -5 % 3 = 1 and 5 % -3 = -1, unlike Rust's truncated modulo.
        assert_eq!(apply("local x = -5 % 3"), "local x=1");
        assert_eq!(apply("local x = 5 % -3"), "local x=-1");
    }

    #[test]
    fn keeps_negative_zero() {
        assert_eq!(apply("local x = 0.0 * -1"), "local x=-0.");
    }

    #[test]
    fn folds_int_min_mixed_with_a_float_only_where_exact() {
        // `0x8000000000000000` wraps to i64::MIN, whose f64 image is exact, so
        // it folds. Its neighbor has no exact image and must not.
        for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
            for (source, expected) in [
                ("return 0x8000000000000000 + 1.5\n", "return-9223372036854776e3"),
                ("return 1.5 + 0x8000000000000000\n", "return-9223372036854776e3"),
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
        // 5.1's floor formula really does produce 0.0, so mirror it.
        assert_eq!(apply_version("return 1e16 % 3\n", LuaVersion::Lua54), "return 1.");
        assert_eq!(apply_version("return 1e16 % 3\n", LuaVersion::Lua51), "return 0");
    }

    #[test]
    fn folds_float_modulo_keeping_the_sign_of_zero_from_lua53() {
        // fmod(-6.0, 3.0) is -0.0 under 5.3+; the floor formula loses
        // the sign. Observable via 1/x and tostring.
        assert_eq!(apply_version("return -6.0 % 3.0\n", LuaVersion::Lua54), "return-0.");
        assert_eq!(apply_version("return -6.0 % 3.0\n", LuaVersion::Lua51), "return 0");
    }

    #[test]
    fn subtracting_integer_zero_matches_the_lua54_immediate_add() {
        // 5.4+ compiles `x - 0` to OP_ADDI with -0, and -0.0 + 0.0 is +0.0.
        for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
            let result = apply_version("return -0.0 - 0\n", version);
            assert_ne!(result, "return-0.", "{version:?} evaluates this to +0.0");
        }
    }

    #[test]
    fn folds_integer_powers_with_an_exact_result() {
        for (source, expected) in [
            ("return 2 ^ 10\n", "return 1024."),
            ("return (-2) ^ 3\n", "return-8."),
            ("return 7 ^ 0\n", "return 1."),
            ("return 2 ^ 53\n", "return 2^53"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_exponentiation_with_an_inexact_result() {
        // An inexact `^` depends on the target's C pow.
        assert_eq!(apply("return 0.3 ^ 3\n"), "return.3^3");
    }

    #[test]
    fn folds_a_run_from_the_side_it_groups_from() {
        for (source, expected) in [
            // Left-associative: the folded prefix stays the run's first.
            ("return 1 + 2 + x + 3", "return 3+x+3"),
            // Right-associative: the folded suffix stays the run's last.
            ("return x .. \"a\" .. \"b\" .. 'c'", "return x..\"abc\""),
            ("return \"a\" .. \"b\" .. x", "return\"a\"..\"b\"..x"),
            ("return \"a\" .. (\"b\") .. \"\\\"\"", "return'ab\"'"),
            ("return 2 ^ 2 ^ 3", "return 256."),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_a_double_negation_through_parentheses() {
        let config = TransformConfig { simplify_parens: true, ..FOLDING };
        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            assert_eq!(
                minify_with("return -(-0x625)", version, &config),
                "return 1573",
                "{version:?}"
            );
        }
    }

    #[test]
    fn a_negated_numeral_is_already_folded() {
        for source in ["return-1", "return-1.5", "return-0xffffffffffffffff"] {
            assert_eq!(apply(source), source);
        }
        assert_eq!(apply("return - -1"), "return 1");
    }
}
