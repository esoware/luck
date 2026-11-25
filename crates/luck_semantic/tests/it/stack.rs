//! Analysis walks the whole tree, and the language server analyzes what's
//! typed as it's typed, broken or not.

use luck_allocator::Allocator;
use luck_semantic::SemanticBuilder;
use luck_testgen::{
    broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_past_the_nesting_cap,
};

#[test]
fn analyzing_fits_the_smallest_stack() {
    let sources = deep_and_long_sources()
        .into_iter()
        .chain(sources_past_the_nesting_cap())
        .chain(broken_sources());
    on_the_smallest_stack(sources, |source, version| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes(), version);
        let chunk = allocator.alloc(parsed.chunk);
        let built = SemanticBuilder::new()
            .with_counts(parsed.counts)
            .with_check(true)
            .with_nodes(true)
            .with_requires(true)
            .build(chunk);
        assert!(!built.semantic.scoping().scopes().is_empty(), "the chunk has a scope");
    });
}

/// A long list of locals holds more than any Lua compiles in one function,
/// which is all the checker finds.
#[test]
fn checking_valid_deep_and_long_sources_finds_only_their_excess_of_locals() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes(), version);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors.first());
        let chunk = allocator.alloc(parsed.chunk);
        let built = SemanticBuilder::new().with_check(true).build(chunk);
        let errors: Vec<&str> = built.errors.iter().map(|diag| &*diag.message).collect();
        assert!(
            errors.iter().all(|message| message.starts_with("too many local variables")),
            "the source is valid but for its length: {errors:?}"
        );
    });
}
