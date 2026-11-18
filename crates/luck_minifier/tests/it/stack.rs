//! Every walk recurses once per level, so input nested to the parser's cap,
//! and runs and lists far longer than it, have to minify on
//! `luck_ast::MIN_STACK_BYTES`.

use luck_allocator::Allocator;
use luck_minifier::TransformConfig;
use luck_testgen::{deep_and_long_sources, on_the_smallest_stack};

#[test]
fn minifying_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let allocator = Allocator::new();
        let config = TransformConfig::default();
        let minified =
            luck_minifier::minify(&allocator, source, version, &config).expect("the source parses");
        let again = luck_minifier::minify(&allocator, &minified, version, &config)
            .expect("the output parses");
        assert!(again == minified, "minifying is idempotent");
    });
}
