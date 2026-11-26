use luck_allocator::Allocator;
use luck_ast::Block;
use luck_stdlib::{SecurityLevel, StdlibLibrary, luau_library, roblox_library};

/// `source`'s tree, which must parse cleanly, kept in `allocator` so it
/// outlives the analysis that borrows it.
pub(crate) fn parse<'ast>(allocator: &'ast Allocator, source: &'ast str) -> &'ast Block<'ast> {
    let parsed = luck_parser::parse(allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{source}: {:?}", parsed.errors);
    allocator.alloc(parsed.chunk)
}

/// Luau's library, or Roblox's over it for a script at the default level.
pub(crate) fn library(has_roblox: bool) -> &'static StdlibLibrary<'static> {
    if has_roblox { roblox_library(SecurityLevel::None) } else { luau_library() }
}
