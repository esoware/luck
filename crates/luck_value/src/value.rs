use std::cmp::Ordering;

use luck_token::{BinOp, LuaNumber, LuaVersion};

use crate::arithmetic::{ArithmeticOp, BitwiseOp, arithmetic, bitwise, compare, number_equals};

/// A value an expression can be known to evaluate to. Each has no identity,
/// as a table or a closure has, so two of them are the same value when
/// they're equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstantValue<'ast> {
    Nil,
    Boolean(bool),
    Number(LuaNumber),
    /// Luau: a 64-bit integer, which `1i` spells.
    Integer(i64),
    /// A string's bytes, which need not be UTF-8.
    String(&'ast [u8]),
}

impl ConstantValue<'_> {
    /// Whether the value is truthy, which every value but `nil` and `false`
    /// is.
    pub const fn to_boolean(self) -> bool {
        !matches!(self, Self::Nil | Self::Boolean(false))
    }

    pub const fn as_number(self) -> Option<LuaNumber> {
        if let Self::Number(number) = self { Some(number) } else { None }
    }

    /// `self op rhs` in `version`'s exact arithmetic, or `None` when `op`
    /// raises on these values or the target's result isn't exactly known.
    ///
    /// Strings compare through the C locale's `strcoll` and a power that
    /// isn't an exact integer goes through the C library's `pow`, so neither
    /// has a result here. Nor does `..`, which makes a new string, and spells
    /// a number the target's own way.
    pub fn binary(self, op: BinOp, rhs: Self, version: LuaVersion) -> Option<Self> {
        match op {
            BinOp::And => Some(if self.to_boolean() { rhs } else { self }),
            BinOp::Or => Some(if self.to_boolean() { self } else { rhs }),
            BinOp::Eq => Some(Self::Boolean(self.raw_equals(rhs))),
            BinOp::Ne => Some(Self::Boolean(!self.raw_equals(rhs))),
            BinOp::Lt => self.compare(rhs, Ordering::is_lt),
            BinOp::Le => self.compare(rhs, Ordering::is_le),
            BinOp::Gt => self.compare(rhs, Ordering::is_gt),
            BinOp::Ge => self.compare(rhs, Ordering::is_ge),
            BinOp::Add => self.arithmetic(ArithmeticOp::Add, rhs, version),
            BinOp::Sub => self.arithmetic(ArithmeticOp::Sub, rhs, version),
            BinOp::Mul => self.arithmetic(ArithmeticOp::Mul, rhs, version),
            BinOp::Div => self.arithmetic(ArithmeticOp::Div, rhs, version),
            // Lua 5.3+, Luau
            BinOp::FloorDiv => self.arithmetic(ArithmeticOp::FloorDiv, rhs, version),
            BinOp::Mod => self.arithmetic(ArithmeticOp::Mod, rhs, version),
            BinOp::Pow => self.arithmetic(ArithmeticOp::Pow, rhs, version),
            // Lua 5.3+
            BinOp::BitAnd => self.bitwise(BitwiseOp::And, rhs, version),
            // Lua 5.3+
            BinOp::BitOr => self.bitwise(BitwiseOp::Or, rhs, version),
            // Lua 5.3+
            BinOp::BitXor => self.bitwise(BitwiseOp::Xor, rhs, version),
            // Lua 5.3+
            BinOp::Shl => self.bitwise(BitwiseOp::Shl, rhs, version),
            // Lua 5.3+
            BinOp::Shr => self.bitwise(BitwiseOp::Shr, rhs, version),
            BinOp::Concat => None,
        }
    }

    fn compare(self, rhs: Self, holds: fn(Ordering) -> bool) -> Option<Self> {
        compare(self.as_number()?, rhs.as_number()?, holds).map(Self::Boolean)
    }

    fn arithmetic(self, op: ArithmeticOp, rhs: Self, version: LuaVersion) -> Option<Self> {
        arithmetic(self.as_number()?, op, rhs.as_number()?, version).map(Self::Number)
    }

    fn bitwise(self, op: BitwiseOp, rhs: Self, version: LuaVersion) -> Option<Self> {
        if !version.has_bitwise_ops() {
            return None;
        }
        let lhs = self.as_number()?.to_integer()?;
        let integer = bitwise(lhs, op, rhs.as_number()?.to_integer()?);
        Some(Self::Number(LuaNumber::Int(integer)))
    }

    /// Lua's `==` with no metamethod, which is all a value without identity
    /// takes: two values of different types are never equal, and numbers are
    /// equal by their exact value.
    fn raw_equals(self, other: Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Boolean(lhs), Self::Boolean(rhs)) => lhs == rhs,
            (Self::Number(lhs), Self::Number(rhs)) => number_equals(lhs, rhs),
            (Self::Integer(lhs), Self::Integer(rhs)) => lhs == rhs,
            (Self::String(lhs), Self::String(rhs)) => lhs == rhs,
            (
                Self::Nil | Self::Boolean(_) | Self::Number(_) | Self::Integer(_) | Self::String(_),
                _,
            ) => false,
        }
    }
}
