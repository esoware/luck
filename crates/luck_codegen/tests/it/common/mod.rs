use luck_allocator::Allocator;
use luck_ast::Chunk;
use luck_testgen::tree_shape;
use luck_token::LuaVersion;

pub(crate) fn assert_compacts_to(source: &str, expected: &str, version: LuaVersion) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source, version);
    assert!(parsed.errors.is_empty(), "parse errors for {source:?}: {:?}", parsed.errors);
    assert_eq!(luck_codegen::compact(&parsed.chunk), expected, "input: {source:?}");
    assert_reprints(&parsed.chunk);
}

/// `chunk` compacts to output that parses back to the same tree and compacts
/// to itself. Output that parses to a different tree can still compact to
/// itself, so the second check alone would miss it.
pub(crate) fn assert_reprints(chunk: &Chunk<'_>) {
    let output = luck_codegen::compact(chunk);
    let allocator = Allocator::new();
    let reparsed = luck_parser::parse(&allocator, &output, chunk.version);
    assert!(
        reparsed.errors.is_empty(),
        "compact output {output:?} doesn't re-parse: {:?}",
        reparsed.errors.first()
    );
    assert_eq!(
        tree_shape(&reparsed.chunk),
        tree_shape(chunk),
        "compact output {output:?} parses to a different tree"
    );
    assert_eq!(luck_codegen::compact(&reparsed.chunk), output, "compacting again changed it");
}
