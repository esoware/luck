//! Input nested to the cap, past it, and broken at every level has to parse
//! on `luck_ast::MIN_STACK_BYTES`.

use luck_allocator::Allocator;
use luck_testgen::{
    broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_past_the_nesting_cap,
};

#[test]
fn parsing_past_the_cap_fits_the_smallest_stack() {
    on_the_smallest_stack(sources_past_the_nesting_cap(), |source| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes());
        let [error] = parsed.errors.as_slice() else {
            panic!("{:?}: {:?}", source.get(..20), parsed.errors.first());
        };
        assert!(error.message.contains("resource limit"), "{:?}: {error:?}", source.get(..20));
    });
}

#[test]
fn parsing_broken_input_fits_the_smallest_stack() {
    on_the_smallest_stack(broken_sources(), |source| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes());
        assert!(!parsed.errors.is_empty(), "{:?}", source.get(..20));
    });
}

#[test]
fn parsing_and_comparing_fit_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes());
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors.first());
        let other_allocator = Allocator::new();
        let again = luck_parser::parse(&other_allocator, source.as_bytes());
        assert!(again.chunk.block == parsed.chunk.block, "a reparse equals the first parse");
    });
}
