use luck_token::{LuaNumber, LuaVersion, NumberSubtypes, parse_lua_number};

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
    assert_eq!(parse_lua_number("1e3", NumberSubtypes::IntFloat), Some(LuaNumber::Float(1000.0)));
    assert_eq!(parse_lua_number("2E2", NumberSubtypes::IntFloat), Some(LuaNumber::Float(200.0)));
    assert_eq!(parse_lua_number("1.5e-1", NumberSubtypes::IntFloat), Some(LuaNumber::Float(0.15)));
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

#[test]
fn bug_parse_lua_number_refuses_float_word_spellings() {
    for text in ["inf", "Infinity", "nan", "NaN"] {
        for subtypes in [NumberSubtypes::IntFloat, NumberSubtypes::Unified] {
            assert_eq!(
                parse_lua_number(text, subtypes),
                None,
                "{text} is a name in Lua, not a numeral ({subtypes:?})"
            );
        }
    }
}

#[test]
fn bug_parse_lua_number_refuses_signed_text() {
    for text in ["+1", "-1", "+1.5", "0x+1"] {
        for subtypes in [NumberSubtypes::IntFloat, NumberSubtypes::Unified] {
            assert_eq!(
                parse_lua_number(text, subtypes),
                None,
                "{text} is not a numeral, because Lua literals carry no sign ({subtypes:?})"
            );
        }
    }
}
