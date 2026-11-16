use core::ops::Range;

use luck_formatter::{FormatOptions, format, format_range};
use luck_token::LuaVersion;

/// `input` formatted, after checking that it parses, that the output formats
/// to itself, and that the output re-parses clean.
pub(crate) fn formatted_with(input: &str, version: LuaVersion, options: &FormatOptions) -> String {
    let result = format(input, version, options);
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
    let second_pass = format(&result.output, version, options);
    assert_eq!(second_pass.output, result.output, "not idempotent");
    let reparsed = luck_parser::parse(&result.output, version);
    assert!(reparsed.errors.is_empty(), "formatted output has parse errors: {:?}", reparsed.errors);
    result.output
}

pub(crate) fn assert_format(input: &str, expected: &str) {
    assert_format_with(input, expected, LuaVersion::Lua54, &FormatOptions::default());
}

pub(crate) fn assert_format_with(
    input: &str,
    expected: &str,
    version: LuaVersion,
    options: &FormatOptions,
) {
    assert_eq!(formatted_with(input, version, options), expected, "format mismatch");
}

/// Nothing outside the selected statements changes, so the second pass finds
/// them in the output at the same distance from each end of the text.
pub(crate) fn assert_format_range(input: &str, range: Range<usize>, expected: &str) {
    let version = LuaVersion::Lua54;
    let options = FormatOptions::default();
    let Range { start, end } = range;
    let result = format_range(input, version, &options, start..end);
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
    assert_eq!(result.output, expected, "format mismatch");
    let formatted_end = expected.len().saturating_sub(input.len().saturating_sub(end));
    let second_pass = format_range(expected, version, &options, start..formatted_end);
    assert_eq!(second_pass.output, expected, "not idempotent");
    let reparsed = luck_parser::parse(expected, version);
    assert!(reparsed.errors.is_empty(), "formatted output has parse errors: {:?}", reparsed.errors);
}
