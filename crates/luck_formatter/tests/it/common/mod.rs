use luck_formatter::{FormatOptions, format};
use luck_token::LuaVersion;

pub fn assert_format(input: &str, expected: &str) {
    assert_format_with(
        input,
        expected,
        LuaVersion::Lua54,
        &FormatOptions::default(),
    );
}

pub fn assert_format_with(
    input: &str,
    expected: &str,
    version: LuaVersion,
    options: &FormatOptions,
) {
    let result = format(input, version, options);
    assert!(
        result.errors.is_empty(),
        "parse errors: {:?}",
        result.errors
    );
    assert_eq!(result.output, expected, "format mismatch");
    let result2 = format(&result.output, version, options);
    assert_eq!(result2.output, expected, "not idempotent");
    let reparse = luck_parser::parse(&result.output, version);
    assert!(
        reparse.errors.is_empty(),
        "formatted output has parse errors: {:?}",
        reparse.errors
    );
}
