//! The formatter recurses once per level, so a tree at the cap, and runs and
//! lists far longer than it, have to format on `luck_ast::MIN_STACK_BYTES`.

use luck_allocator::Allocator;
use luck_formatter::{FormatOptions, format};
use luck_testgen::{deep_and_long_sources, on_the_smallest_stack};

#[test]
fn formatting_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source| {
        let allocator = Allocator::new();
        let options = FormatOptions::default();
        let formatted = format(&allocator, source.as_bytes(), &options)
            .unwrap_or_else(|errors| panic!("{:?}", errors.first()));
        let reparsed = luck_parser::parse(&allocator, &formatted);
        assert!(reparsed.errors.is_empty(), "the output reparses");
        let again = format(&allocator, &formatted, &options);
        assert!(again.as_ref() == Ok(&formatted), "formatting is idempotent");
    });
}
