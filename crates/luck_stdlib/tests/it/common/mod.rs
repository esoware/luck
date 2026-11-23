use luck_allocator::Allocator;
use luck_ast::Chunk;
use luck_stdlib::{StdlibLibrary, library_for};
use luck_token::{LuaTarget, LuaVersion};

/// `source`'s tree, which must parse cleanly, kept in `allocator` so it
/// outlives the analysis that borrows it.
pub(crate) fn parse<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
) -> &'ast Chunk<'ast> {
    let parsed = luck_parser::parse(allocator, source.as_bytes(), version);
    assert!(parsed.errors.is_empty(), "{source}: {:?}", parsed.errors);
    allocator.alloc(parsed.chunk)
}

pub(crate) const NUMBERED_VERSIONS: [LuaVersion; 5] =
    [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55];

pub(crate) fn standalone_library(version: LuaVersion) -> &'static StdlibLibrary {
    library_for(version.into())
}

pub(crate) fn luau() -> &'static StdlibLibrary {
    standalone_library(LuaVersion::Luau)
}

pub(crate) fn roblox() -> &'static StdlibLibrary {
    library_for(LuaTarget::LuauRoblox)
}

pub(crate) fn all_libraries() -> [(&'static str, &'static StdlibLibrary); 7] {
    [
        ("lua51", standalone_library(LuaVersion::Lua51)),
        ("lua52", standalone_library(LuaVersion::Lua52)),
        ("lua53", standalone_library(LuaVersion::Lua53)),
        ("lua54", standalone_library(LuaVersion::Lua54)),
        ("lua55", standalone_library(LuaVersion::Lua55)),
        ("luau", luau()),
        ("luau_roblox", roblox()),
    ]
}
