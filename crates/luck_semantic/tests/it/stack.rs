//! Analysis walks the whole tree, and the language server analyzes what's
//! typed as it's typed, broken or not.

use luck_allocator::Allocator;
use luck_testgen::{
    broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_past_the_nesting_cap,
};
use luck_token::LuaTarget;

#[test]
fn analyzing_fits_the_smallest_stack() {
    let sources = deep_and_long_sources()
        .into_iter()
        .chain(sources_past_the_nesting_cap())
        .chain(broken_sources());
    on_the_smallest_stack(sources, |source, version| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes(), version);
        let counts = parsed.counts;
        let chunk = allocator.alloc(parsed.chunk);
        for target in LuaTarget::ALL.into_iter().filter(|target| target.version() == version) {
            let analysis = luck_semantic::analyze(chunk, counts, target);
            assert!(!analysis.scope_tree().scopes().is_empty(), "the chunk has a scope");
        }
    });
}
