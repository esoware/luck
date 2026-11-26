use luck_allocator::Allocator;
use luck_ast::Block;
use luck_codegen::LiteralSpelling;
use luck_parser::ParseResult;
use luck_testgen::tree_shape;

/// A parse of code, as `luck_parser::parse`, or of a definition file.
pub(crate) type Parse = for<'ast> fn(&'ast Allocator, &'ast [u8]) -> ParseResult<'ast>;

pub(crate) fn assert_compacts_to(source: &str, expected: &str) {
    assert_spells(source, expected, LiteralSpelling::AsWritten);
}

pub(crate) fn assert_spells(source: &str, expected: &str, spelling: LiteralSpelling) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "parse errors for {source:?}: {:?}", parsed.errors);
    let output = String::from_utf8(luck_codegen::compact(&parsed.chunk, spelling)).unwrap();
    assert_eq!(output, expected, "input: {source:?}, {spelling:?}");
    assert_reprints(&parsed.chunk);
}

/// `chunk` compacts, with either spelling, to output that reads back right.
///
/// That's output that parses back to the same tree and compacts to itself.
/// Output that parses to a different tree can still compact to itself, so
/// the second check alone would miss it.
pub(crate) fn assert_reprints(chunk: &Block<'_>) {
    assert_reprints_as(chunk, luck_parser::parse);
}

/// [`assert_reprints`], reading the output back with `parse`.
pub(crate) fn assert_reprints_as(chunk: &Block<'_>, parse: Parse) {
    for spelling in [LiteralSpelling::AsWritten, LiteralSpelling::Shortest] {
        let output = luck_codegen::compact(chunk, spelling);
        let allocator = Allocator::new();
        let reparsed = parse(&allocator, &output);
        let text = String::from_utf8_lossy(&output);
        assert!(
            reparsed.errors.is_empty(),
            "compact output {text:?} doesn't re-parse: {:?}",
            reparsed.errors.first()
        );
        assert_eq!(
            tree_shape(&reparsed.chunk),
            tree_shape(chunk),
            "compact output {text:?} parses to a different tree"
        );
        assert_eq!(
            luck_codegen::compact(&reparsed.chunk, spelling),
            output,
            "compacting again changed it"
        );
    }
}
