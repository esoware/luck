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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_subtypes_split_only_from_lua53() {
        for (version, subtypes) in [
            (LuaVersion::Lua51, NumberSubtypes::Unified),
            (LuaVersion::Lua52, NumberSubtypes::Unified),
            (LuaVersion::Lua53, NumberSubtypes::IntFloat),
            (LuaVersion::Lua54, NumberSubtypes::IntFloat),
            (LuaVersion::Lua55, NumberSubtypes::IntFloat),
            (LuaVersion::Luau, NumberSubtypes::Unified),
        ] {
            assert_eq!(NumberSubtypes::from(version), subtypes, "{version:?}");
        }
    }

    #[test]
    fn parse_lua_number_reads_decimal_ints_and_floats() {
        assert_eq!(parse_lua_number("1", NumberSubtypes::IntFloat), Some(LuaNumber::Int(1)));
        assert_eq!(parse_lua_number("42", NumberSubtypes::IntFloat), Some(LuaNumber::Int(42)));
        assert_eq!(parse_lua_number("1.5", NumberSubtypes::IntFloat), Some(LuaNumber::Float(1.5)));
        assert_eq!(parse_lua_number("0.0", NumberSubtypes::IntFloat), Some(LuaNumber::Float(0.0)));
    }

    #[test]
    fn parse_lua_number_keeps_integer_one_apart_from_float_one() {
        assert_ne!(LuaNumber::Int(1), LuaNumber::Float(1.0));
        assert_eq!(
            parse_lua_number("1", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(1)),
            "1 must fold as an integer under IntFloat"
        );
        assert_eq!(
            parse_lua_number("1.0", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(1.0)),
            "1.0 must fold as a float"
        );
    }

    #[test]
    fn parse_lua_number_reads_every_number_as_a_float_under_unified() {
        assert_eq!(parse_lua_number("1", NumberSubtypes::Unified), Some(LuaNumber::Float(1.0)));
        assert_eq!(parse_lua_number("42", NumberSubtypes::Unified), Some(LuaNumber::Float(42.0)));
    }

    #[test]
    fn parse_lua_number_reads_exponents_in_either_case_as_floats() {
        assert_eq!(
            parse_lua_number("1e3", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(1000.0))
        );
        assert_eq!(
            parse_lua_number("2E2", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(200.0))
        );
        assert_eq!(
            parse_lua_number("1.5e-1", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(0.15))
        );
        for text in ["1e3", "1E3", "1.0e+3", "1.0E+3"] {
            for subtypes in [NumberSubtypes::IntFloat, NumberSubtypes::Unified] {
                assert_eq!(
                    parse_lua_number(text, subtypes),
                    Some(LuaNumber::Float(1000.0)),
                    "{text} {subtypes:?}"
                );
            }
        }
    }

    #[test]
    fn parse_lua_number_reads_hex_integers_in_either_case_only_under_int_float() {
        assert_eq!(parse_lua_number("0x1A", NumberSubtypes::IntFloat), Some(LuaNumber::Int(26)));
        assert_eq!(parse_lua_number("0xff", NumberSubtypes::IntFloat), Some(LuaNumber::Int(255)));
        assert_eq!(parse_lua_number("0X10", NumberSubtypes::IntFloat), Some(LuaNumber::Int(16)));
        for text in ["0xDeAd", "0XDEAD", "0XdEaD"] {
            assert_eq!(
                parse_lua_number(text, NumberSubtypes::IntFloat),
                Some(LuaNumber::Int(0xdead)),
                "{text}"
            );
            assert_eq!(parse_lua_number(text, NumberSubtypes::Unified), None, "{text}");
        }
    }

    #[test]
    fn parse_lua_number_wraps_hex_integers_into_the_i64_range() {
        // Lua 5.3+ hex integers wrap: u64 max reinterprets as -1.
        assert_eq!(
            parse_lua_number("0xffffffffffffffff", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(-1))
        );
        assert_eq!(
            parse_lua_number("0XFFFFFFFFFFFFFFFF", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(-1))
        );
        // 2^63 reinterprets as i64::MIN.
        assert_eq!(
            parse_lua_number("0x8000000000000000", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(i64::MIN))
        );
        // Past the u64 range, nothing folds.
        assert_eq!(parse_lua_number("0x10000000000000000", NumberSubtypes::IntFloat), None);
        assert_eq!(parse_lua_number("0X10000000000000000", NumberSubtypes::IntFloat), None);
    }

    #[test]
    fn parse_lua_number_refuses_hex_floats() {
        for text in ["0x1.8p1", "0x1p4", "0X1P4", "0x1.8P1", "0X1.8p1"] {
            for subtypes in [NumberSubtypes::IntFloat, NumberSubtypes::Unified] {
                assert_eq!(parse_lua_number(text, subtypes), None, "{text} {subtypes:?}");
            }
        }
    }

    #[test]
    fn parse_lua_number_promotes_overflowing_decimals_to_floats() {
        let parsed = parse_lua_number("99999999999999999999", NumberSubtypes::IntFloat);
        assert!(matches!(parsed, Some(LuaNumber::Float(_))));
    }

    #[test]
    fn parse_lua_number_refuses_malformed_text() {
        assert_eq!(parse_lua_number("abc", NumberSubtypes::IntFloat), None);
        assert_eq!(parse_lua_number("", NumberSubtypes::IntFloat), None);
        assert_eq!(parse_lua_number("1.2.3", NumberSubtypes::IntFloat), None);
    }
}
