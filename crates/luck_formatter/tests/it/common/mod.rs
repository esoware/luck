use std::ops::Range;

use luck_allocator::Allocator;
use luck_ast::{SpanCounter, Synth};
use luck_formatter::{FormatOptions, format_parsed, format_range};
use luck_parser::ParseResult;

/// A parse that keeps its tokens, of code, as
/// `luck_parser::parse_with_tokens`, or of a definition file.
pub(crate) type Parse = for<'ast> fn(&'ast Allocator, &'ast [u8]) -> ParseResult<'ast>;

/// An arena and a span counter for building trees in code.
#[derive(Default)]
pub(crate) struct Fixture {
    allocator: Allocator,
    spans: SpanCounter,
}

impl Fixture {
    pub(crate) fn synth(&self) -> Synth<'_> {
        Synth::new(&self.allocator, &self.spans)
    }

    pub(crate) const fn allocator(&self) -> &Allocator {
        &self.allocator
    }
}

/// `input` formatted, after checking that it parses, that the output formats
/// to itself, and that the output re-parses clean.
pub(crate) fn formatted_with(input: &[u8], options: &FormatOptions) -> Vec<u8> {
    formatted_as(luck_parser::parse_with_tokens, input, options)
}

/// [`formatted_with`], for a source `parse` reads.
pub(crate) fn formatted_as(parse: Parse, input: &[u8], options: &FormatOptions) -> Vec<u8> {
    let allocator = Allocator::new();
    let output = format_parsed(&allocator, &parse(&allocator, input), options)
        .unwrap_or_else(|errors| panic!("parse errors: {errors:?}"));
    let reparsed = parse(&allocator, &output);
    assert!(reparsed.errors.is_empty(), "formatted output has parse errors: {:?}", reparsed.errors);
    let second_pass = format_parsed(&allocator, &reparsed, options);
    assert_eq!(second_pass.as_ref(), Ok(&output), "not idempotent");
    output
}

pub(crate) fn assert_format(input: &str, expected: &str) {
    assert_format_with(input, expected, &FormatOptions::default());
}

/// `input` and `expected` are text or bytes, as a source need not be UTF-8.
pub(crate) fn assert_format_with<Input, Expected>(
    input: &Input,
    expected: &Expected,
    options: &FormatOptions,
) where
    Input: AsRef<[u8]> + ?Sized,
    Expected: AsRef<[u8]> + ?Sized,
{
    let output = formatted_with(input.as_ref(), options);
    let expected_bytes = expected.as_ref();
    let text = String::from_utf8_lossy(&output);
    assert_eq!(text, String::from_utf8_lossy(expected_bytes), "format mismatch");
    assert_eq!(output, expected_bytes, "format mismatch in bytes that aren't UTF-8");
}

/// `source` parsed and range-formatted over `range`.
fn range_formatted(source: &str, range: Range<usize>) -> Vec<u8> {
    let allocator = Allocator::new();
    let parse = luck_parser::parse_with_tokens(&allocator, source.as_bytes());
    format_range(&allocator, &parse, &FormatOptions::default(), range)
        .unwrap_or_else(|errors| panic!("parse errors: {errors:?}"))
}

/// Nothing outside the selected statements changes, so the second pass finds
/// them in the output at the same distance from each end of the text.
pub(crate) fn assert_format_range(input: &str, range: Range<usize>, expected: &str) {
    let Range { start, end } = range;
    assert_eq!(range_formatted(input, start..end), expected.as_bytes(), "format mismatch");
    let formatted_end = expected.len() - (input.len() - end);
    let second_pass = range_formatted(expected, start..formatted_end);
    assert_eq!(second_pass, expected.as_bytes(), "not idempotent");
    let allocator = Allocator::new();
    let reparsed = luck_parser::parse(&allocator, expected.as_bytes());
    assert!(reparsed.errors.is_empty(), "formatted output has parse errors: {:?}", reparsed.errors);
}
