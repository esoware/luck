use luck_token::LuaVersion;

/// Parses `source` under `version`, which must succeed, and compacts it.
pub(crate) fn compact_source(source: &str, version: LuaVersion) -> String {
    let parsed = luck_parser::parse(source, version);
    assert!(parsed.errors.is_empty(), "parse errors for {source:?}: {:?}", parsed.errors);
    luck_codegen::compact(&parsed.block)
}

/// Asserts that `source` compacts to exactly `expected`, and that the output
/// re-parses clean.
pub(crate) fn assert_compacts_to(source: &str, expected: &str, version: LuaVersion) {
    let output = compact_source(source, version);
    assert_eq!(output, expected, "input: {source:?}");
    let reparsed = luck_parser::parse(&output, version);
    assert!(
        reparsed.errors.is_empty(),
        "compact output {output:?} doesn't re-parse: {:?}",
        reparsed.errors
    );
}
