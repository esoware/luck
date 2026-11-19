//! The printer recurses once per level, so a tree at the cap, or whatever a
//! parse past it or of broken input leaves, has to print on
//! `luck_ast::MIN_STACK_BYTES`.

use luck_allocator::Allocator;
use luck_testgen::{
    broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_past_the_nesting_cap,
};

#[test]
fn compacting_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source, version);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors.first());
        let output = luck_codegen::compact(&parsed.chunk);
        let reparsed = luck_parser::parse(&allocator, &output, version);
        assert!(reparsed.errors.is_empty(), "the output reparses");
    });
}

#[test]
fn compacting_what_a_failed_parse_leaves_fits_the_smallest_stack() {
    let sources = sources_past_the_nesting_cap().into_iter().chain(broken_sources());
    on_the_smallest_stack(sources, |source, version| {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source, version);
        assert!(!parsed.errors.is_empty(), "{:?}", source.get(..20));
        let _output = luck_codegen::compact(&parsed.chunk);
    });
}
