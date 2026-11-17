use luck_allocator::Allocator;
use luck_token::LuaVersion;

/// A second compaction that changes the output catches output that re-parses
/// with a different meaning.
pub(crate) fn assert_compacts_to(source: &str, expected: &str, version: LuaVersion) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source, version);
    assert!(parsed.errors.is_empty(), "parse errors for {source:?}: {:?}", parsed.errors);
    let output = luck_codegen::compact(&parsed.block);
    assert_eq!(output, expected, "input: {source:?}");
    assert_reprints(&output, version);
}

/// `output` parses cleanly and compacts to itself.
pub(crate) fn assert_reprints(output: &str, version: LuaVersion) {
    let allocator = Allocator::new();
    let reparsed = luck_parser::parse(&allocator, output, version);
    assert!(
        reparsed.errors.is_empty(),
        "compact output {output:?} doesn't re-parse: {:?}",
        reparsed.errors.first()
    );
    assert_eq!(luck_codegen::compact(&reparsed.block), output, "compacting again changed it");
}
