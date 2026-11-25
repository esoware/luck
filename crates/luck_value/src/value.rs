use std::cmp::Ordering;

use luck_token::BinOp;

/// A value an expression can be known to evaluate to. Each has no identity,
/// as a table or a closure has, so two of them are the same value when
/// they're equal.
///
/// Two are equal when they're the same value of the same type, bit for bit,
/// as two literals are, which is stricter than Luau's `==`: `0.0` and `-0.0`
/// differ, and NaN equals itself.
#[derive(Debug, Clone, Copy)]
pub enum ConstantValue<'ast> {
    Nil,
    Boolean(bool),
    Number(f64),
    /// A string's bytes, which need not be UTF-8.
    String(&'ast [u8]),
}

impl PartialEq for ConstantValue<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Boolean(lhs), Self::Boolean(rhs)) => lhs == rhs,
            (Self::Number(lhs), Self::Number(rhs)) => lhs.to_bits() == rhs.to_bits(),
            (Self::String(lhs), Self::String(rhs)) => lhs == rhs,
            (Self::Nil | Self::Boolean(_) | Self::Number(_) | Self::String(_), _) => false,
        }
    }
}

impl Eq for ConstantValue<'_> {}

impl ConstantValue<'_> {
    /// Whether the value is truthy, which every value but `nil` and `false`
    /// is.
    pub const fn to_boolean(self) -> bool {
        !matches!(self, Self::Nil | Self::Boolean(false))
    }

    pub const fn as_number(self) -> Option<f64> {
        if let Self::Number(number) = self { Some(number) } else { None }
    }

    /// `self op rhs`, or `None` when `op` raises on these values or Luau's
    /// result isn't exactly known.
    ///
    /// Strings compare through the C locale's `strcoll` and a power that
    /// isn't an exact integer goes through the C library's `pow`, so neither
    /// has a result here. Nor does `..`, which makes a new string, and spells
    /// a number Luau's own way.
    pub fn binary(self, op: BinOp, rhs: Self) -> Option<Self> {
        match op {
            BinOp::And => Some(if self.to_boolean() { rhs } else { self }),
            BinOp::Or => Some(if self.to_boolean() { self } else { rhs }),
            BinOp::Eq => Some(Self::Boolean(self.raw_equals(rhs))),
            BinOp::Ne => Some(Self::Boolean(!self.raw_equals(rhs))),
            BinOp::Lt => self.compare(rhs, Ordering::is_lt),
            BinOp::Le => self.compare(rhs, Ordering::is_le),
            BinOp::Gt => self.compare(rhs, Ordering::is_gt),
            BinOp::Ge => self.compare(rhs, Ordering::is_ge),
            BinOp::Add => self.arithmetic(rhs, |lhs, rhs| lhs + rhs),
            BinOp::Sub => self.arithmetic(rhs, |lhs, rhs| lhs - rhs),
            BinOp::Mul => self.arithmetic(rhs, |lhs, rhs| lhs * rhs),
            BinOp::Div => self.arithmetic(rhs, |lhs, rhs| lhs / rhs),
            BinOp::FloorDiv => self.arithmetic(rhs, |lhs, rhs| (lhs / rhs).floor()),
            // `luai_nummod`, whose remainder takes the divisor's sign.
            BinOp::Mod => self.arithmetic(rhs, |lhs, rhs| lhs - (lhs / rhs).floor() * rhs),
            BinOp::Pow => exact_power(self.as_number()?, rhs.as_number()?).map(Self::Number),
            BinOp::Concat => None,
        }
    }

    /// Whether two numbers order as `holds` asks. NaN is ordered with
    /// nothing, so every comparison with it is false.
    fn compare(self, rhs: Self, holds: fn(Ordering) -> bool) -> Option<Self> {
        let ordering = self.as_number()?.partial_cmp(&rhs.as_number()?);
        Some(Self::Boolean(ordering.is_some_and(holds)))
    }

    fn arithmetic(self, rhs: Self, op: fn(f64, f64) -> f64) -> Option<Self> {
        Some(Self::Number(op(self.as_number()?, rhs.as_number()?)))
    }

    /// Luau's `==` with no metamethod, which is all a value without identity
    /// takes: two values of different types are never equal, and numbers are
    /// equal by their value, so `0.0 == -0.0` and NaN equals nothing.
    fn raw_equals(self, other: Self) -> bool {
        match (self, other) {
            (Self::Number(lhs), Self::Number(rhs)) => {
                lhs.partial_cmp(&rhs) == Some(Ordering::Equal)
            }
            (Self::Nil | Self::Boolean(_) | Self::Number(_) | Self::String(_), _) => self == other,
        }
    }
}

/// `base ^ exponent` when both are integers and every partial product is
/// exact. Anything else goes through the C library's `pow`, whose rounding
/// isn't ours to reproduce.
fn exact_power(base: f64, exponent: f64) -> Option<f64> {
    const EXACT_LIMIT: f64 = 9_007_199_254_740_992.0;
    if base.fract() != 0.0 || exponent.fract() != 0.0 || !(0.0..=53.0).contains(&exponent) {
        return None;
    }
    let mut power = 1.0;
    let mut remaining = exponent;
    while remaining >= 1.0 {
        power *= base;
        if power.abs() >= EXACT_LIMIT {
            return None;
        }
        remaining -= 1.0;
    }
    Some(power)
}
