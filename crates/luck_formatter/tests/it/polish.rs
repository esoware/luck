//! The options that shape output beyond basic layout.
//!
//! Number case, `block_newline_gaps`, `sort_requires`,
//! `space_after_function_names`, `CallParentheses::Input`, format ignore,
//! `magic_trailing_comma`, and verification.

use luck_formatter::{
    BlockNewlineGaps, CallParentheses, FormatOptions, HexCase, SpaceAfterFunction, format,
    format_and_verify,
};
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with};

#[test]
fn hex_prefix_lowered_by_default() {
    // The default, `Preserve`, keeps digit case but still lowercases `0X`.
    assert_format_with(
        "local x = 0XB0\n",
        "local x = 0xB0\n",
        LuaVersion::Luau,
        &FormatOptions::default(),
    );
}

#[test]
fn hex_digits_lowered_when_configured() {
    let options = FormatOptions { hexadecimal_case: HexCase::Lower, ..FormatOptions::default() };
    assert_format_with("local x = 0XDEad\n", "local x = 0xdead\n", LuaVersion::Luau, &options);
}

#[test]
fn hex_digits_uppered_when_configured() {
    let options = FormatOptions { hexadecimal_case: HexCase::Upper, ..FormatOptions::default() };
    assert_format_with("local x = 0xdead\n", "local x = 0xDEAD\n", LuaVersion::Luau, &options);
}

#[test]
fn decimal_exponent_lowered() {
    assert_format_with(
        "local x = 1E10\n",
        "local x = 1e10\n",
        LuaVersion::Lua54,
        &FormatOptions::default(),
    );
}

#[test]
fn number_normalization_verifies() {
    // The AST-equivalence oracle must accept the case rewrite.
    let options = FormatOptions { hexadecimal_case: HexCase::Upper, ..FormatOptions::default() };
    let result = format_and_verify("local x = 0Xb0 + 1e5\n", LuaVersion::Luau, &options);
    assert!(result.is_ok(), "number normalization broke verification");
}

#[test]
fn block_gaps_never_strips_leading_blank() {
    // The default, `Never`, strips blank lines at the top of a `do` body.
    let input = "do\n\n\tlocal x = 1\nend\n";
    let expected = "do\n\tlocal x = 1\nend\n";
    assert_format(input, expected);
}

#[test]
fn block_gaps_preserve_keeps_leading_blank() {
    let options = FormatOptions {
        block_newline_gaps: BlockNewlineGaps::Preserve,
        ..FormatOptions::default()
    };
    let input = "do\n\n\tlocal x = 1\nend\n";
    let expected = "do\n\n\tlocal x = 1\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_keeps_trailing_blank() {
    let options = FormatOptions {
        block_newline_gaps: BlockNewlineGaps::Preserve,
        ..FormatOptions::default()
    };
    let input = "do\n\tlocal x = 1\n\nend\n";
    let expected = "do\n\tlocal x = 1\n\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_function_body() {
    let options = FormatOptions {
        block_newline_gaps: BlockNewlineGaps::Preserve,
        ..FormatOptions::default()
    };
    let input = "local function f()\n\n\tlocal x = 1\n\nend\n";
    let expected = "local function f()\n\n\tlocal x = 1\n\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_then_branch() {
    let options = FormatOptions {
        block_newline_gaps: BlockNewlineGaps::Preserve,
        ..FormatOptions::default()
    };
    let input = "if x then\n\n\tlocal y = 1\nend\n";
    let expected = "if x then\n\n\tlocal y = 1\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_off_by_default() {
    let input = "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n";
    let result = format(input, LuaVersion::Lua54, &FormatOptions::default());
    assert_eq!(result.output, input);
}

#[test]
fn sort_requires_alphabetizes_consecutive() {
    let options = FormatOptions { sort_requires: true, ..FormatOptions::default() };
    let input = "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\n";
    let expected = "local alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\nlocal zeta = require(\"zeta\")\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_respects_blank_line_break() {
    let options = FormatOptions { sort_requires: true, ..FormatOptions::default() };
    // A blank line splits the run into two single requires, so nothing is
    // reordered.
    let input = "local zeta = require(\"zeta\")\n\nlocal alpha = require(\"alpha\")\n";
    let expected = "local zeta = require(\"zeta\")\n\nlocal alpha = require(\"alpha\")\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_verifies_against_the_sorted_buffer() {
    // Verification compares the output to the text the formatter read, which
    // `sort_requires` rewrote. Comparing against the original would report
    // every reordering as a structural change.
    let options = FormatOptions { sort_requires: true, ..FormatOptions::default() };
    let input = "-- a header\nlocal zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\nreturn alpha, zeta\n";
    let result = format_and_verify(input, LuaVersion::Lua54, &options)
        .unwrap_or_else(|(_, diff)| panic!("{diff:?}"));
    assert_eq!(
        result.output,
        "-- a header\nlocal alpha = require(\"alpha\")\nlocal zeta = require(\"zeta\")\nreturn alpha, zeta\n"
    );
}

#[test]
fn sort_requires_format_off_excluded() {
    let options = FormatOptions { sort_requires: true, ..FormatOptions::default() };
    let input = "-- luck: format off\nlocal zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n-- luck: format on\n";
    let result = format(input, LuaVersion::Lua54, &options);
    assert_eq!(result.errors, Vec::<luck_token::SourceError>::new());
    // `sort_requires` must not reorder inside a format-off region, which goes
    // out as written.
    assert!(result.output.contains("local zeta"));
    assert!(result.output.contains("local alpha"));
    let zeta_offset = result.output.find("local zeta").unwrap();
    let alpha_offset = result.output.find("local alpha").unwrap();
    assert!(
        zeta_offset < alpha_offset,
        "format-off region must preserve source order: got\n{}",
        result.output
    );
}

#[test]
fn space_after_function_never_default() {
    assert_format_with(
        "local function f() return 1 end\n",
        "local function f()\n\treturn 1\nend\n",
        LuaVersion::Lua54,
        &FormatOptions::default(),
    );
}

#[test]
fn space_after_function_definitions_only() {
    let options = FormatOptions {
        space_after_function_names: SpaceAfterFunction::Definitions,
        ..FormatOptions::default()
    };
    assert_format_with(
        "local function f() return 1 end\nf()\n",
        "local function f ()\n\treturn 1\nend\nf()\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn space_after_function_calls_only() {
    let options = FormatOptions {
        space_after_function_names: SpaceAfterFunction::Calls,
        ..FormatOptions::default()
    };
    assert_format_with(
        "local function f() return 1 end\nf()\n",
        "local function f()\n\treturn 1\nend\nf ()\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn space_after_function_always() {
    let options = FormatOptions {
        space_after_function_names: SpaceAfterFunction::Always,
        ..FormatOptions::default()
    };
    assert_format_with(
        "local function f() return 1 end\nf()\n",
        "local function f ()\n\treturn 1\nend\nf ()\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn call_parens_input_preserves_bare_string() {
    let options =
        FormatOptions { call_parentheses: CallParentheses::Input, ..FormatOptions::default() };
    assert_format_with("print\"hi\"\n", "print \"hi\"\n", LuaVersion::Lua54, &options);
}

#[test]
fn call_parens_input_preserves_parenthesized_string() {
    let options =
        FormatOptions { call_parentheses: CallParentheses::Input, ..FormatOptions::default() };
    assert_format_with("print(\"hi\")\n", "print(\"hi\")\n", LuaVersion::Lua54, &options);
}

#[test]
fn call_parens_input_preserves_bare_table() {
    let options =
        FormatOptions { call_parentheses: CallParentheses::Input, ..FormatOptions::default() };
    assert_format_with("print{1, 2}\n", "print { 1, 2 }\n", LuaVersion::Lua54, &options);
}

#[test]
fn format_ignore_alias_keeps_next_stmt_verbatim() {
    // `luck: format ignore` is an alias of `luck: ignore`.
    let input = "-- luck: format ignore\nlocal   x   =   1\nlocal y = 2\n";
    let result = format(input, LuaVersion::Lua54, &FormatOptions::default());
    assert!(result.errors.is_empty(), "parse: {:?}", result.errors);
    assert!(
        result.output.contains("local   x   =   1"),
        "ignored statement must be verbatim, got:\n{}",
        result.output
    );
    assert!(
        result.output.contains("local y = 2"),
        "subsequent statements must be formatted, got:\n{}",
        result.output
    );
}

#[test]
fn magic_trailing_comma_off_keeps_packed_table() {
    // With `magic_trailing_comma` off, a trailing comma doesn't force a break,
    // so the table stays packed.
    let input = "local t = { 1, 2, 3, }\n";
    let result = format(input, LuaVersion::Lua54, &FormatOptions::default());
    assert_eq!(result.errors, Vec::<luck_token::SourceError>::new());
    assert!(!result.output.contains("\n\t1,"));
}

#[test]
fn magic_trailing_comma_on_expands_table() {
    // The expanded output keeps its trailing comma, so the idempotency check
    // finds the second pass multi-line too.
    let options = FormatOptions { magic_trailing_comma: true, ..FormatOptions::default() };
    let input = "local t = { 1, 2, 3, }\n";
    let expected = "local t = {\n\t1,\n\t2,\n\t3,\n}\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn magic_trailing_comma_on_expands_call_args_via_table() {
    // No dialect allows a trailing comma after the last argument, so the
    // option can only reach a call through a table argument.
    let options = FormatOptions { magic_trailing_comma: true, ..FormatOptions::default() };
    let input = "f({ 1, 2, 3, })\n";
    let result = format(input, LuaVersion::Lua54, &options);
    assert!(result.errors.is_empty(), "parse: {:?}", result.errors);
    assert!(
        result.output.contains("\n\t1,"),
        "table inside call must expand, got:\n{}",
        result.output
    );
}

#[test]
fn verify_passes_on_clean_format() {
    let input = "local x = 1\nlocal y = 2\n";
    let result = format_and_verify(input, LuaVersion::Lua54, &FormatOptions::default())
        .expect("structurally equivalent format");
    assert_eq!(result.errors, Vec::<luck_token::SourceError>::new());
}
