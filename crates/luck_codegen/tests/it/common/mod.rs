use luck_token::LuaVersion;

pub fn roundtrip_compact(source: &str, version: LuaVersion) -> String {
    let result = luck_parser::parse(source, version);
    assert!(
        result.errors.is_empty(),
        "parse errors for {:?}: {:?}",
        source,
        result.errors
    );
    luck_codegen::compact(&result.block, source)
}

pub fn verify_roundtrip(source: &str, expected: &str, version: LuaVersion) {
    let output = roundtrip_compact(source, version);
    assert_eq!(output, expected, "input: {:?}", source);
    let reparsed = luck_parser::parse(&output, version);
    assert!(
        reparsed.errors.is_empty(),
        "compact output {:?} doesn't re-parse: {:?}",
        output,
        reparsed.errors
    );
}

pub fn v51(source: &str, expected: &str) {
    verify_roundtrip(source, expected, LuaVersion::Lua51);
}

pub fn v54(source: &str, expected: &str) {
    verify_roundtrip(source, expected, LuaVersion::Lua54);
}

pub fn v55(source: &str, expected: &str) {
    verify_roundtrip(source, expected, LuaVersion::Lua55);
}
