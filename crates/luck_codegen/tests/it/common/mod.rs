use luck_allocator::Allocator;
use luck_ast::Block;
use luck_testgen::tree_shape;
use luck_token::LuaVersion;

pub(crate) fn assert_compacts_to(source: &str, expected: &str, version: LuaVersion) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source, version);
    assert!(parsed.errors.is_empty(), "parse errors for {source:?}: {:?}", parsed.errors);
    assert_eq!(luck_codegen::compact(&parsed.block), expected, "input: {source:?}");
    assert_reprints(&parsed.block, version);
}

/// `block` compacts to output that parses back to the same tree and compacts
/// to itself. Output that parses to a different tree can still compact to
/// itself, so the second check alone would miss it.
pub(crate) fn assert_reprints(block: &Block<'_>, version: LuaVersion) {
    let output = luck_codegen::compact(block);
    let allocator = Allocator::new();
    let reparsed = luck_parser::parse(&allocator, &output, version);
    assert!(
        reparsed.errors.is_empty(),
        "compact output {output:?} doesn't re-parse: {:?}",
        reparsed.errors.first()
    );
    assert_eq!(
        tree_shape(&reparsed.block, version),
        tree_shape(block, version),
        "compact output {output:?} parses to a different tree"
    );
    assert_eq!(luck_codegen::compact(&reparsed.block), output, "compacting again changed it");
}
