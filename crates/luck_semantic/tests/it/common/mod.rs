use luck_allocator::Allocator;
use luck_ast::Chunk;
use luck_semantic::{Semantic, SemanticBuilder};
use luck_testgen::{generate_full_grammar, generate_runtime_safe, luau_fixtures};

pub(crate) const NAMES_OF_EVERY_KIND: &str = "\
local x, y = 1
print(x, t.x)
t[x], x = x, function(x) y = x.y end
function t:m(a) return self, a end
for i = 1, 2 do local z: typeof(i) = i end";

/// `source`'s tree, which must parse cleanly, kept in `allocator` so it
/// outlives the node table that borrows it.
pub(crate) fn parse<'ast>(allocator: &'ast Allocator, source: &'ast str) -> &'ast Chunk<'ast> {
    let parsed = luck_parser::parse(allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
    allocator.alloc(parsed.chunk)
}

pub(crate) fn semantic_for<'ast>(allocator: &'ast Allocator, source: &'ast str) -> Semantic<'ast> {
    SemanticBuilder::new()
        .with_nodes(true)
        .with_requires(true)
        .build(parse(allocator, source))
        .semantic
}

/// Runs `check` on every fixture that isn't an error fixture, and on
/// generated programs.
pub(crate) fn for_each_valid_source(mut check: impl FnMut(&str)) {
    let mut checked = 0usize;
    for fixture in luau_fixtures() {
        if fixture.is_error {
            continue;
        }
        check(&fixture.source);
        checked += 1;
    }
    assert!(checked > 0, "no fixture outside `errors/`");
    for seed in 0..40 {
        check(&generate_full_grammar(seed, 25));
        check(&generate_runtime_safe(seed, 25));
    }
}
