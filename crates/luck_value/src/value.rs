use luck_token::{BinOp, LuaNumber, LuaVersion};

use crate::arithmetic::{arithmetic, bitwise, compare, number_equals};

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
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                compare(self.as_number()?, op, rhs.as_number()?).map(Self::Boolean)
            }
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::FloorDiv
            | BinOp::Mod
            | BinOp::Pow => {
                arithmetic(self.as_number()?, op, rhs.as_number()?, version).map(Self::Number)
            }
            // Lua 5.3+
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                if !version.has_bitwise_ops() {
                    return None;
                }
                let lhs = self.as_number()?.to_integer()?;
                let integer = bitwise(lhs, op, rhs.as_number()?.to_integer()?)?;
                Some(Self::Number(LuaNumber::Int(integer)))
            }
            BinOp::Concat => None,
        }
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

    pub(crate) const fn ty(self) -> ValueType {
        match self {
            Self::Nil => ValueType::Nil,
            Self::Boolean(_) => ValueType::Boolean,
            Self::Number(_) => ValueType::Number,
            Self::Integer(_) => ValueType::Integer,
            Self::String(_) => ValueType::String,
        }
    }
}

/// The type of a value an expression yields, as far as the analysis tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueType {
    Nil,
    Boolean,
    Number,
    String,
    Table,
    Function,
    /// Luau: a 64-bit integer, which no arithmetic operator takes.
    Integer,
    /// Any type: a variable's value, or a metamethod's result.
    Undetermined,
}

impl ValueType {
    /// Whether a value of this type is truthy, when the type tells.
    pub(crate) const fn truthiness(self) -> Option<bool> {
        match self {
            Self::Nil => Some(false),
            Self::Number | Self::String | Self::Table | Self::Function | Self::Integer => {
                Some(true)
            }
            Self::Boolean | Self::Undetermined => None,
        }
    }

    /// Whether a value of this type may be a table or userdata, the only
    /// values whose `__eq` runs, and only when both operands are.
    pub(crate) const fn may_have_eq_metamethod(self) -> bool {
        match self {
            Self::Table | Self::Undetermined => true,
            Self::Nil
            | Self::Boolean
            | Self::Number
            | Self::String
            | Self::Function
            | Self::Integer => false,
        }
    }
}
