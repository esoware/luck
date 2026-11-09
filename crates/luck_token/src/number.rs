use crate::LuaVersion;

/// A compile-time Lua number carrying the 5.3+ integer/float subtype.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LuaNumber {
    Int(i64),
    Float(f64),
}

/// Whether the target dialect distinguishes integer and float number subtypes.
///
/// Lua 5.3+ does ([`LuaVersion::has_integer_subtype`]), while 5.1, 5.2, and
/// Luau model every number as a single f64. Under `Unified`,
/// [`parse_lua_number`] never returns [`LuaNumber::Int`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberSubtypes {
    Unified,
    IntFloat,
}

impl From<LuaVersion> for NumberSubtypes {
    fn from(version: LuaVersion) -> Self {
        if version.has_integer_subtype() { Self::IntFloat } else { Self::Unified }
    }
}

/// The value of a number literal token's raw text.
///
/// Returns `None` for any spelling this doesn't model exactly, such as a hex
/// float, and a caller that gets `None` must not fold.
#[must_use]
pub fn parse_lua_number(text: &str, subtypes: NumberSubtypes) -> Option<LuaNumber> {
    if subtypes == NumberSubtypes::Unified {
        return text.parse::<f64>().ok().map(LuaNumber::Float);
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        if hex.contains(['.', 'p', 'P']) {
            // Hex floats are rare, and never folded.
            return None;
        }
        // Lua 5.3+: hex integer literals wrap into the integer range.
        return u64::from_str_radix(hex, 16).ok().map(|value| LuaNumber::Int(value.cast_signed()));
    }
    if text.contains(['.', 'e', 'E']) {
        return text.parse::<f64>().ok().map(LuaNumber::Float);
    }
    // Overflowing decimal integer literals become floats in 5.3+.
    text.parse::<i64>()
        .ok()
        .map(LuaNumber::Int)
        .or_else(|| text.parse::<f64>().ok().map(LuaNumber::Float))
}
