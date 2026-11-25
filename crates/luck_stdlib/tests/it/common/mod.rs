use luck_allocator::Allocator;
use luck_ast::Block;
use luck_stdlib::{StdlibLibrary, Variant, expand_replace_template, library_for};

/// `source`'s tree, which must parse cleanly, kept in `allocator` so it
/// outlives the analysis that borrows it.
pub(crate) fn parse<'ast>(allocator: &'ast Allocator, source: &'ast str) -> &'ast Block<'ast> {
    let parsed = luck_parser::parse(allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{source}: {:?}", parsed.errors);
    allocator.alloc(parsed.chunk)
}

/// `template` expanded with `args`, which the tests spell as text.
pub(crate) fn expand(template: &str, args: &[&str]) -> String {
    let arg_bytes: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
    String::from_utf8(expand_replace_template(template, &arg_bytes)).expect("text expands to text")
}

pub(crate) fn luau() -> &'static StdlibLibrary<'static> {
    library_for(Variant::Standalone)
}

pub(crate) fn roblox() -> &'static StdlibLibrary<'static> {
    library_for(Variant::Roblox)
}

pub(crate) fn all_libraries() -> [(Variant, &'static StdlibLibrary<'static>); 2] {
    Variant::ALL.map(|variant| (variant, library_for(variant)))
}
