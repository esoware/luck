//! Lua's operators on numbers, in each target's exact arithmetic. An
//! operation whose result the target computes in a way this can't reproduce
//! has no result here.

use core::cmp::Ordering;

use luck_token::{BinOp, LuaNumber, LuaVersion};

/// `lhs op rhs` for an arithmetic `op`, or `None` for any other operator, an
/// integer `%` or `//` by zero, which raises, or a result the target doesn't
/// compute exactly.
pub(crate) fn arithmetic(
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
            BinOp::Div | BinOp::Pow => float_arithmetic(lhs.to_f64(), op, rhs.to_f64(), version),
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
        (Float(left), Float(right)) => float_arithmetic(left, op, right, version),
        (Int(left), Float(right)) => float_arithmetic(exact_f64(left)?, op, right, version),
        (Float(left), Int(right)) => {
            // 5.4+ compiles `x - 0` as an immediate add, so -0.0 keeps or
            // loses its sign depending on the target.
            if op == BinOp::Sub && right == 0 && left == 0.0f64 && left.is_sign_negative() {
                return None;
            }
            float_arithmetic(left, op, exact_f64(right)?, version)
        }
    }
}

/// `-number`. Integers wrap, so `i64::MIN` negates to itself.
#[expect(clippy::float_arithmetic, reason = "this is Lua unary minus, evaluated at compile time")]
pub(crate) fn negate(number: LuaNumber) -> LuaNumber {
    match number {
        LuaNumber::Int(value) => LuaNumber::Int(value.wrapping_neg()),
        LuaNumber::Float(value) => LuaNumber::Float(-value),
    }
}

/// `lhs op rhs` for an order comparison `op`, or `None` for any other
/// operator or a mixed pair this can't compare exactly. NaN is ordered with
/// nothing, so every comparison with it is false.
pub(crate) fn compare(lhs: LuaNumber, op: BinOp, rhs: LuaNumber) -> Option<bool> {
    use LuaNumber::{Float, Int};
    let ordering = match (lhs, rhs) {
        (Int(left), Int(right)) => Some(left.cmp(&right)),
        (Float(left), Float(right)) => left.partial_cmp(&right),
        // Lua compares an integer with a float exactly, which a double does
        // only for an integer it holds exactly.
        (Int(left), Float(right)) => exact_f64(left)?.partial_cmp(&right),
        (Float(left), Int(right)) => left.partial_cmp(&exact_f64(right)?),
    };
    Some(match op {
        BinOp::Lt => ordering == Some(Ordering::Less),
        BinOp::Le => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        BinOp::Gt => ordering == Some(Ordering::Greater),
        BinOp::Ge => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::Concat
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::And
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => return None,
    })
}

/// Lua's `==` on two numbers, which is exact: an integer equals a float only
/// of its own value, and NaN equals nothing.
pub(crate) fn number_equals(lhs: LuaNumber, rhs: LuaNumber) -> bool {
    use LuaNumber::{Float, Int};
    match (lhs, rhs) {
        (Int(left), Int(right)) => left == right,
        (Float(left), Float(right)) => left.partial_cmp(&right) == Some(Ordering::Equal),
        (Int(integer), Float(float)) | (Float(float), Int(integer)) => {
            Float(float).to_integer() == Some(integer)
        }
    }
}

/// `lhs op rhs` for a Lua 5.3+ bitwise `op` on the integers its operands
/// convert to, or `None` for any other operator.
pub(crate) fn bitwise(lhs: i64, op: BinOp, rhs: i64) -> Option<i64> {
    match op {
        BinOp::BitAnd => Some(lhs & rhs),
        BinOp::BitOr => Some(lhs | rhs),
        BinOp::BitXor => Some(lhs ^ rhs),
        BinOp::Shl => Some(shift_left(lhs, rhs)),
        BinOp::Shr => Some(shift_left(lhs, rhs.wrapping_neg())),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::Concat
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => None,
    }
}

/// `luaV_shiftl`: a logical shift, rightward for a negative `shift`, which
/// leaves 0 once it moves every bit out.
fn shift_left(value: i64, shift: i64) -> i64 {
    let bits = value.cast_unsigned();
    let shifted = if shift >= 0 {
        u32::try_from(shift).ok().and_then(|count| bits.checked_shl(count))
    } else {
        u32::try_from(shift.unsigned_abs()).ok().and_then(|count| bits.checked_shr(count))
    };
    shifted.unwrap_or(0).cast_signed()
}

/// The integer as a double, when the double holds it exactly.
///
/// That's within 2^53 of zero. `i64::MIN` has no positive counterpart, so
/// `wrapping_abs` leaves it negative and it passes, which is right, since
/// -2^63 is a power of two.
fn exact_f64(value: i64) -> Option<f64> {
    (value.wrapping_abs() <= (1i64 << 53)).then(|| LuaNumber::Int(value).to_f64())
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

/// `luaV_mod`: the remainder takes the divisor's sign, and `% 0` raises.
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

/// `luaV_idiv`: the quotient rounds toward negative infinity, `// 0` raises,
/// and `i64::MIN // -1` wraps to `i64::MIN`.
fn floored_div(lhs: i64, rhs: i64) -> Option<i64> {
    if rhs == -1 {
        return Some(lhs.wrapping_neg());
    }
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
fn float_arithmetic(lhs: f64, op: BinOp, rhs: f64, version: LuaVersion) -> Option<LuaNumber> {
    let value = match op {
        BinOp::Add => lhs + rhs,
        BinOp::Sub => lhs - rhs,
        BinOp::Mul => lhs * rhs,
        BinOp::Div => lhs / rhs,
        BinOp::Mod => {
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
        BinOp::FloorDiv => (lhs / rhs).floor(),
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
        | BinOp::Shr => return None,
    };
    Some(LuaNumber::Float(value))
}
