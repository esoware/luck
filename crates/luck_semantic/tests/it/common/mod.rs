use luck_allocator::Allocator;
use luck_ast::Chunk;
use luck_semantic::{Semantic, SemanticBuilder};
use luck_testgen::{generate_full_grammar, generate_runtime_safe, lua_fixtures};
use luck_token::LuaVersion;

pub(crate) const NAMES_OF_EVERY_KIND: &str = "\
local x, y = 1
print(x, t.x)
t[x], x = x, function(x) y = x.y end
function t:m(a) return self, a end
for i = 1, 2 do goto done end
::done::";

/// `source`'s tree, which must parse cleanly, kept in `allocator` so it
/// outlives the node table that borrows it.
pub(crate) fn parse<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
) -> &'ast Chunk<'ast> {
    let parsed = luck_parser::parse(allocator, source.as_bytes(), version);
    assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
    allocator.alloc(parsed.chunk)
}

pub(crate) fn semantic_for<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
) -> Semantic<'ast> {
    SemanticBuilder::new()
        .with_nodes(true)
        .with_requires(true)
        .build(parse(allocator, source, version))
        .semantic
}

/// Runs `check` on every fixture that isn't an error fixture, as the
/// version its path names, and on generated programs of every version.
pub(crate) fn for_each_valid_source(mut check: impl FnMut(&str, LuaVersion)) {
    let mut checked = 0usize;
    for fixture in lua_fixtures() {
        if fixture.is_error {
            continue;
        }
        check(&fixture.source, fixture.version);
        checked += 1;
    }
    assert!(checked > 0, "no fixture outside `errors/`");
    for version in LuaVersion::ALL {
        for seed in 0..40 {
            check(&generate_full_grammar(seed, version, 25), version);
            check(&generate_runtime_safe(seed, version, 25), version);
        }
    }
}
