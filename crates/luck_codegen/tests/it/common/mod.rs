use luck_token::LuaVersion;

/// A second compaction that changes the output catches output that re-parses
/// with a different meaning.
pub(crate) fn assert_compacts_to(source: &str, expected: &str, version: LuaVersion) {
    let parsed = luck_parser::parse(source, version);
    assert!(parsed.errors.is_empty(), "parse errors for {source:?}: {:?}", parsed.errors);
    let output = luck_codegen::compact(&parsed.block);
    assert_eq!(output, expected, "input: {source:?}");
    let reparsed = luck_parser::parse(&output, version);
    assert!(
        reparsed.errors.is_empty(),
        "compact output {output:?} doesn't re-parse: {:?}",
        reparsed.errors
    );
    assert_eq!(
        luck_codegen::compact(&reparsed.block),
        output,
        "compact output of {source:?} changed when compacted again"
    );
}
