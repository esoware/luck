use core::ops::Range;

use luck_allocator::Allocator;
use luck_ast::{SpanCounter, Synth};
use luck_formatter::{FormatOptions, FormatResult, format, format_range};
use luck_token::LuaVersion;

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
pub(crate) fn formatted_with(
    input: &[u8],
    version: LuaVersion,
    options: &FormatOptions,
) -> Vec<u8> {
    let allocator = Allocator::new();
    let result = format(&allocator, input, version, options);
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
    let second_pass = format(&allocator, &result.output, version, options);
    assert_eq!(second_pass.output, result.output, "not idempotent");
    let reparsed = luck_parser::parse(&allocator, &result.output, version);
    assert!(reparsed.errors.is_empty(), "formatted output has parse errors: {:?}", reparsed.errors);
    result.output
}

pub(crate) fn assert_format(input: &str, expected: &str) {
    assert_format_with(input, expected, LuaVersion::Lua54, &FormatOptions::default());
}

/// `input` and `expected` are text or bytes, as a source need not be UTF-8.
pub(crate) fn assert_format_with<Input, Expected>(
    input: &Input,
    expected: &Expected,
    version: LuaVersion,
    options: &FormatOptions,
) where
    Input: AsRef<[u8]> + ?Sized,
    Expected: AsRef<[u8]> + ?Sized,
{
    let output = formatted_with(input.as_ref(), version, options);
    let expected_bytes = expected.as_ref();
    let text = String::from_utf8_lossy(&output);
    assert_eq!(text, String::from_utf8_lossy(expected_bytes), "format mismatch");
    assert_eq!(output, expected_bytes, "format mismatch in bytes that aren't UTF-8");
}

/// `source` parsed and range-formatted over `range`.
fn range_formatted(source: &str, range: Range<usize>) -> FormatResult {
    let allocator = Allocator::new();
    let version = LuaVersion::Lua54;
    let parse = luck_parser::parse_with_tokens(&allocator, source.as_bytes(), version);
    format_range(&allocator, &parse, &FormatOptions::default(), range)
}

/// Nothing outside the selected statements changes, so the second pass finds
/// them in the output at the same distance from each end of the text.
pub(crate) fn assert_format_range(input: &str, range: Range<usize>, expected: &str) {
    let Range { start, end } = range;
    let result = range_formatted(input, start..end);
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
    assert_eq!(result.output, expected.as_bytes(), "format mismatch");
    let formatted_end = expected.len().saturating_sub(input.len().saturating_sub(end));
    let second_pass = range_formatted(expected, start..formatted_end);
    assert_eq!(second_pass.output, expected.as_bytes(), "not idempotent");
    let allocator = Allocator::new();
    let reparsed = luck_parser::parse(&allocator, expected.as_bytes(), LuaVersion::Lua54);
    assert!(reparsed.errors.is_empty(), "formatted output has parse errors: {:?}", reparsed.errors);
}
