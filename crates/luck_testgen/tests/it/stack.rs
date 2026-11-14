//! Input nested to the parser's cap, and runs and sibling lists far longer
//! than it, have to run through every stage on the smallest stack luck gets
//! embedded on, in an unoptimized build too.

use luck_formatter::{FormatOptions, format};
use luck_minifier::TransformConfig;
use luck_testgen::{
    broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_past_the_nesting_cap,
};

#[test]
fn parsing_past_the_cap_fits_the_smallest_stack() {
    on_the_smallest_stack(sources_past_the_nesting_cap(), |source, version| {
        let parsed = luck_parser::parse(source, version);
        let [error] = parsed.errors.as_slice() else {
            panic!("{:?}: {:?}", source.get(..20), parsed.errors.first());
        };
        assert!(error.message.contains("resource limit"), "{:?}: {error:?}", source.get(..20));
    });
}

#[test]
fn parsing_broken_input_fits_the_smallest_stack() {
    on_the_smallest_stack(broken_sources(), |source, version| {
        let parsed = luck_parser::parse(source, version);
        assert!(!parsed.errors.is_empty(), "{:?}", source.get(..20));
    });
}

#[test]
fn parsing_validating_and_dropping_fit_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors.first());
        let cloned = parsed.block.clone();
        assert!(cloned == parsed.block, "a clone equals its original");
        assert!(luck_parser::validate(&parsed.block, version).is_empty(), "the source is valid");
    });
}

#[test]
fn compact_printing_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let parsed = luck_parser::parse(source, version);
        let output = luck_codegen::compact(&parsed.block).expect("a parsed tree prints");
        assert!(luck_parser::parse(output, version).errors.is_empty(), "the output reparses");
    });
}

#[test]
fn formatting_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let options = FormatOptions::default();
        let formatted = format(source, version, &options);
        assert!(formatted.errors.is_empty(), "{:?}", formatted.errors.first());
        let reparsed = luck_parser::parse(&formatted.output, version);
        assert!(reparsed.errors.is_empty(), "the output reparses");
        let again = format(&formatted.output, version, &options);
        assert!(again.output == formatted.output, "formatting is idempotent");
    });
}

#[test]
fn minifying_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let config = TransformConfig::default();
        let minified = luck_minifier::minify(source, version, &config).expect("the source parses");
        let again = luck_minifier::minify(&minified, version, &config).expect("the output parses");
        assert!(again == minified, "minifying is idempotent");
    });
}
