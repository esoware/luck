use luck_allocator::Allocator;
use luck_ast::Block;
use luck_token::LuaVersion;

/// `source`'s tree, which must parse cleanly, kept in `allocator` so it
/// lives as long as the names it lends an analysis.
pub(crate) fn parse<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
) -> &'ast Block<'ast> {
    let parsed = luck_parser::parse(allocator, source, version);
    assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
    allocator.alloc(parsed.block)
}
