//! Wave 1C polish features: block_newline_gaps, sort_requires,
//! space_after_function_names, CallParentheses::Input, format ignore,
//! magic_trailing_comma, --verify.

use crate::common::assert_format_with;
use luck_formatter::{
    BlockNewlineGaps, CallParentheses, FormatOptions, HexCase, SpaceAfterFunction, format,
    format_and_verify,
};
use luck_token::LuaVersion;

fn opts() -> FormatOptions {
    FormatOptions::default()
}

#[test]
fn hex_prefix_lowered_by_default() {
    // Default Preserve keeps digit case but always fixes the `0X` prefix.
    assert_format_with(
        "local x = 0XB0\n",
        "local x = 0xB0\n",
        LuaVersion::Luau,
        &opts(),
    );
}

#[test]
fn hex_digits_lowered_when_configured() {
    let mut options = opts();
    options.hexadecimal_case = HexCase::Lower;
    assert_format_with(
        "local x = 0XDEad\n",
        "local x = 0xdead\n",
        LuaVersion::Luau,
        &options,
    );
}

#[test]
fn hex_digits_uppered_when_configured() {
    let mut options = opts();
    options.hexadecimal_case = HexCase::Upper;
    assert_format_with(
        "local x = 0xdead\n",
        "local x = 0xDEAD\n",
        LuaVersion::Luau,
        &options,
    );
}

#[test]
fn decimal_exponent_lowered() {
    assert_format_with(
        "local x = 1E10\n",
        "local x = 1e10\n",
        LuaVersion::Lua54,
        &opts(),
    );
}

#[test]
fn number_normalization_verifies() {
    // The AST-equivalence oracle must accept the case rewrite.
    let mut options = opts();
    options.hexadecimal_case = HexCase::Upper;
    let result = format_and_verify("local x = 0Xb0 + 1e5\n", LuaVersion::Luau, &options);
    assert!(result.is_ok(), "number normalization broke verification");
}

#[test]
fn block_gaps_never_strips_leading_blank() {
    // Default Never strips blank lines at the top of a do-block body.
    let input = "do\n\n\tlocal x = 1\nend\n";
    let expected = "do\n\tlocal x = 1\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &opts());
}

#[test]
fn block_gaps_preserve_keeps_leading_blank() {
    let mut options = opts();
    options.block_newline_gaps = BlockNewlineGaps::Preserve;
    let input = "do\n\n\tlocal x = 1\nend\n";
    let expected = "do\n\n\tlocal x = 1\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_keeps_trailing_blank() {
    let mut options = opts();
    options.block_newline_gaps = BlockNewlineGaps::Preserve;
    let input = "do\n\tlocal x = 1\n\nend\n";
    let expected = "do\n\tlocal x = 1\n\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_function_body() {
    let mut options = opts();
    options.block_newline_gaps = BlockNewlineGaps::Preserve;
    let input = "local function f()\n\n\tlocal x = 1\n\nend\n";
    let expected = "local function f()\n\n\tlocal x = 1\n\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_then_branch() {
    let mut options = opts();
    options.block_newline_gaps = BlockNewlineGaps::Preserve;
    let input = "if x then\n\n\tlocal y = 1\nend\n";
    let expected = "if x then\n\n\tlocal y = 1\nend\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_off_by_default() {
    let input = "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n";
    let result = format(input, LuaVersion::Lua54, &opts());
    assert_eq!(result.output, input);
}

#[test]
fn sort_requires_alphabetizes_consecutive() {
    let mut options = opts();
    options.sort_requires = true;
    let input = "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\n";
    let expected = "local alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\nlocal zeta = require(\"zeta\")\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_respects_blank_line_break() {
    let mut options = opts();
    options.sort_requires = true;
    // A blank line splits the run; neither sub-group has 2 entries so nothing
    // is reordered.
    let input = "local zeta = require(\"zeta\")\n\nlocal alpha = require(\"alpha\")\n";
    let expected = "local zeta = require(\"zeta\")\n\nlocal alpha = require(\"alpha\")\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_format_off_excluded() {
    let mut options = opts();
    options.sort_requires = true;
    let input = "-- luck: format off\nlocal zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n-- luck: format on\n";
    let result = format(input, LuaVersion::Lua54, &options);
    assert!(result.errors.is_empty());
    // Inside a format-off block, sort_requires must not reorder; the verbatim
    // emitter will reproduce the lines as written.
    assert!(result.output.contains("local zeta"));
    assert!(result.output.contains("local alpha"));
    let zeta_idx = result.output.find("local zeta").unwrap();
    let alpha_idx = result.output.find("local alpha").unwrap();
    assert!(
        zeta_idx < alpha_idx,
        "format-off region must preserve source order: got\n{}",
        result.output
    );
}

#[test]
fn space_after_fn_never_default() {
    assert_format_with(
        "local function f() return 1 end\n",
        "local function f()\n\treturn 1\nend\n",
        LuaVersion::Lua54,
        &opts(),
    );
}

#[test]
fn space_after_fn_definitions_only() {
    let mut options = opts();
    options.space_after_function_names = SpaceAfterFunction::Definitions;
    assert_format_with(
        "local function f() return 1 end\nf()\n",
        "local function f ()\n\treturn 1\nend\nf()\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn space_after_fn_calls_only() {
    let mut options = opts();
    options.space_after_function_names = SpaceAfterFunction::Calls;
    assert_format_with(
        "local function f() return 1 end\nf()\n",
        "local function f()\n\treturn 1\nend\nf ()\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn space_after_fn_always() {
    let mut options = opts();
    options.space_after_function_names = SpaceAfterFunction::Always;
    assert_format_with(
        "local function f() return 1 end\nf()\n",
        "local function f ()\n\treturn 1\nend\nf ()\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn call_parens_input_preserves_bare_string() {
    let mut options = opts();
    options.call_parentheses = CallParentheses::Input;
    assert_format_with(
        "print\"hi\"\n",
        "print \"hi\"\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn call_parens_input_preserves_parenthesized_string() {
    let mut options = opts();
    options.call_parentheses = CallParentheses::Input;
    assert_format_with(
        "print(\"hi\")\n",
        "print(\"hi\")\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn call_parens_input_preserves_bare_table() {
    let mut options = opts();
    options.call_parentheses = CallParentheses::Input;
    assert_format_with(
        "print{1, 2}\n",
        "print { 1, 2 }\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn format_ignore_preserves_next_statement() {
    let input = "-- luck: format ignore\nlocal   x   =   1\nlocal y = 2\n";
    let result = format(input, LuaVersion::Lua54, &opts());
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
fn legacy_luck_ignore_still_works() {
    let input = "-- luck: ignore\nlocal   x   =   1\nlocal y = 2\n";
    let result = format(input, LuaVersion::Lua54, &opts());
    assert!(result.errors.is_empty());
    assert!(result.output.contains("local   x   =   1"));
    assert!(result.output.contains("local y = 2"));
}

#[test]
fn magic_trailing_comma_off_keeps_packed_table() {
    // Default magic_trailing_comma=false: trailing comma does NOT force expand.
    let input = "local t = { 1, 2, 3, }\n";
    let result = format(input, LuaVersion::Lua54, &opts());
    assert!(result.errors.is_empty());
    // The fill/flat layout is allowed when magic_trailing_comma is off.
    assert!(!result.output.contains("\n\t1,"));
}

#[test]
fn magic_trailing_comma_on_expands_table() {
    let mut options = opts();
    options.magic_trailing_comma = true;
    let input = "local t = { 1, 2, 3, }\n";
    let expected = "local t = {\n\t1,\n\t2,\n\t3,\n}\n";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn magic_trailing_comma_on_expands_call_args_via_table() {
    // The parser doesn't accept a bare trailing comma in call args, so we
    // exercise the call-args expansion path through a nested table literal
    // whose trailing comma propagates expand up to the surrounding call.
    let mut options = opts();
    options.magic_trailing_comma = true;
    let input = "f({ 1, 2, 3, })\n";
    let result = format(input, LuaVersion::Lua54, &options);
    assert!(result.errors.is_empty(), "parse: {:?}", result.errors);
    // The table is forced to expand by the magic comma, which propagates the
    // expand decision up through the parent call group.
    assert!(
        result.output.contains("\n\t1,"),
        "table inside call must expand, got:\n{}",
        result.output
    );
}

#[test]
fn magic_trailing_comma_idempotent_after_expand() {
    // After magic expand, the output retains the trailing comma so the next
    // run stays multi-line.
    let mut options = opts();
    options.magic_trailing_comma = true;
    let input = "local t = { 1, 2, 3, }\n";
    let pass1 = format(input, LuaVersion::Lua54, &options);
    assert!(pass1.errors.is_empty());
    let pass2 = format(&pass1.output, LuaVersion::Lua54, &options);
    assert_eq!(pass1.output, pass2.output);
}

#[test]
fn verify_passes_on_clean_format() {
    let input = "local x = 1\nlocal y = 2\n";
    let result = format_and_verify(input, LuaVersion::Lua54, &opts())
        .expect("structurally equivalent format");
    assert!(result.errors.is_empty());
}

#[test]
fn verify_detects_structural_divergence() {
    // We simulate a buggy printer by hand-rolling a "formatted" string with a
    // structural change, then calling blocks_equiv directly on the original
    // and modified parse trees. This proves the diff machinery surfaces the
    // first point of divergence with a useful path.
    let original = luck_parser::parse("local x = 1\nlocal y = 2", LuaVersion::Lua54).block;
    let mutated = luck_parser::parse("local x = 1", LuaVersion::Lua54).block;
    let err = luck_formatter::blocks_equiv(&original, &mutated).expect_err("must diverge");
    assert!(err.path.contains("block"));
    assert!(err.reason.contains("count"));
}

#[test]
fn verify_detects_renamed_identifier() {
    let original = luck_parser::parse("local x = 1", LuaVersion::Lua54).block;
    let mutated = luck_parser::parse("local pr1nt = 1", LuaVersion::Lua54).block;
    let err = luck_formatter::blocks_equiv(&original, &mutated).expect_err("rename must diverge");
    assert!(err.reason.contains("kind"), "got: {}", err.reason);
}
