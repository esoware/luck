use luck_formatter::{FormatOptions, format};
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

/// [`formatted_with`] as Lua 5.4 under the default options.
pub(crate) fn formatted(input: &str) -> String {
    formatted_with(input, LuaVersion::Lua54, &FormatOptions::default())
}

pub(crate) fn assert_format(input: &str, expected: &str) {
    assert_eq!(formatted(input), expected, "format mismatch");
}

pub(crate) fn assert_format_with(
    input: &str,
    expected: &str,
    version: LuaVersion,
    options: &FormatOptions,
) {
    assert_eq!(formatted_with(input, version, options), expected, "format mismatch");
}
