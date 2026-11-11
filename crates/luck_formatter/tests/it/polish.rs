use luck_formatter::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, HexCase,
    SpaceAfterFunction, format, format_and_verify,
};
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with, format_once};

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
fn sort_requires_on_by_default() {
    assert_format(
        "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n",
        "local alpha = require(\"alpha\")\nlocal zeta = require(\"zeta\")\n",
    );
}

#[test]
fn sort_requires_off_keeps_the_source_order() {
    let options = FormatOptions { sort_requires: false, ..FormatOptions::default() };
    let input = "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n";
    assert_format_with(input, input, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_orders_instance_path_requires() {
    assert_format_with(
        "local Zebra = require(script.Parent.Zebra)\nlocal Apple = require(Packages.Apple)\n",
        "local Apple = require(Packages.Apple)\nlocal Zebra = require(script.Parent.Zebra)\n",
        LuaVersion::Luau,
        &FormatOptions::default(),
    );
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
    let options = FormatOptions { magic_trailing_comma: false, ..FormatOptions::default() };
    let input = "local t = { 1, 2, 3, }\n";
    let result = format(input, LuaVersion::Lua54, &options);
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
fn magic_trailing_comma_keeps_a_broken_table_broken_by_default() {
    let input = "local t = {\n\ttype = \"baz\",\n}\n";
    assert_format(input, input);
}

#[test]
fn magic_trailing_comma_keeps_a_packed_list_packed() {
    // The values stay several to a line, repacked to the width, and the
    // table stays broken.
    assert_format(
        "local libs = {\n\t\"roact\", \"rodux\",\n\t\"cryo\",\n}\n",
        "local libs = {\n\t\"roact\", \"rodux\", \"cryo\",\n}\n",
    );
}

#[test]
fn magic_trailing_comma_gives_a_wrapped_fill_its_comma() {
    let options = FormatOptions { line_width: 12, ..FormatOptions::default() };
    assert_format_with(
        "local t = {1,2,3,4,5,6}\n",
        "local t = {\n\t1, 2, 3,\n\t4, 5, 6,\n}\n",
        LuaVersion::Lua54,
        &options,
    );
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

fn sorting_options() -> FormatOptions {
    FormatOptions { sort_requires: true, ..FormatOptions::default() }
}

fn sort_once(input: &str) -> String {
    format_once(input, LuaVersion::Lua54, &sorting_options())
}

fn preserving_gaps_options() -> FormatOptions {
    FormatOptions { block_newline_gaps: BlockNewlineGaps::Preserve, ..FormatOptions::default() }
}

#[test]
fn sort_requires_keeps_a_stmt_before_a_comment_at_eof() {
    let output = sort_once("local z = require(\"z\")\nlocal a = require(\"a\") -- note");
    let reparsed = luck_parser::parse(&output, LuaVersion::Lua54);
    assert_eq!(
        reparsed.block.stmts.len(),
        2,
        "a last line with no newline must not swallow the next sorted line into its comment: {output:?}"
    );
}

#[test]
fn sort_requires_keeps_a_comment_between_requires() {
    let output = sort_once("local b = require(\"b\")\n; -- important\nlocal a = require(\"a\")\n");
    assert!(output.contains("-- important"), "the comment must not be deleted: {output:?}");
}

#[test]
fn sort_requires_keeps_a_multi_line_comment_between_requires() {
    let output = sort_once("local z = require(\"z\") --[[\n]] local a = require(\"a\")\n");
    assert!(output.contains("--[[\n]]"), "the comment must not be split or lost: {output:?}");
}

#[test]
fn sort_requires_keeps_a_require_inside_the_block_opened_on_its_line() {
    let output = sort_once("do local z = require(\"z\")\nlocal a = require(\"a\")\nend\n");
    assert!(
        output.starts_with("do\n"),
        "`local a` must not move out of the `do` block: {output:?}"
    );
}

#[test]
fn sort_requires_keeps_the_end_closing_on_a_require_line() {
    let output = sort_once("if x then\nlocal z = require(\"z\")\nlocal a = require(\"a\") end\n");
    assert!(output.ends_with("end\n"), "`local z` must not move past the `end`: {output:?}");
}

#[test]
fn sort_requires_keeps_a_binding_before_the_get_service_that_uses_it() {
    let input = "local zeta = require(\"z\")\nlocal alpha = zeta:GetService(\"x\")\n";
    assert_eq!(sort_once(input), input, "`alpha` reads `zeta`, so `zeta` must stay first");
}

#[test]
fn sort_requires_keeps_a_shadowing_require_before_its_callers() {
    let input = "local require = require(\"custom\")\nlocal a = require(\"a\")\n";
    assert_eq!(
        sort_once(input),
        input,
        "`local a` calls the local `require`, so it must stay after it"
    );
}

#[test]
fn sort_requires_keeps_an_ignored_stmt_under_its_directive() {
    let output =
        sort_once("-- luck: ignore\nlocal   z  = require(\"z\")\nlocal a = require(\"a\")\n");
    assert!(
        output.starts_with("-- luck: ignore\nlocal   z  = require(\"z\")\n"),
        "`luck: ignore` must keep applying to the statement it was written above: {output:?}"
    );
}

#[test]
fn sort_requires_keeps_the_close_order_of_to_be_closed_variables() {
    let input = "local z <close> = require(\"z\")\nlocal a <close> = require(\"a\")\n";
    assert_eq!(
        sort_once(input),
        input,
        "to-be-closed variables close in reverse declaration order"
    );
}

#[test]
fn magic_trailing_comma_formats_a_wrapped_fill_idempotently() {
    let options =
        FormatOptions { magic_trailing_comma: true, line_width: 12, ..FormatOptions::default() };
    let first_pass = format_once("local t = {1,2,3,4,5,6,7,8,9,10}\n", LuaVersion::Lua54, &options);
    assert_eq!(
        format_once(&first_pass, LuaVersion::Lua54, &options),
        first_pass,
        "the comma a width-wrapped fill adds must not read as a magic trailing comma on the next pass"
    );
}

#[test]
fn block_gaps_preserve_keeps_a_blank_after_a_trailing_comment() {
    let input = "do\n\tx() -- c\n\nend\n";
    assert_eq!(
        format_once(input, LuaVersion::Lua54, &preserving_gaps_options()),
        input,
        "the blank line before `end` must survive a trailing comment on the last statement"
    );
}

#[test]
fn block_gaps_preserve_keeps_a_blank_after_a_semicolon() {
    assert_eq!(
        format_once("do\n\tx();\n\nend\n", LuaVersion::Lua54, &preserving_gaps_options()),
        "do\n\tx()\n\nend\n",
        "the blank line before `end` must survive a dropped `;` on the last statement"
    );
}

#[test]
fn collapse_with_preserved_gaps_keeps_a_valid_layout() {
    let options = FormatOptions {
        collapse_simple_statement: CollapseSimpleStatement::ConditionalOnly,
        ..preserving_gaps_options()
    };
    let output = format_once("if a then\n\n\tx = 1\n\nend\n", LuaVersion::Lua54, &options);
    assert!(
        output == "if a then\n\n\tx = 1\n\nend\n" || output == "if a then x = 1 end\n",
        "the body must be indented or collapsed, not left at column 0 with a ` end` line: {output:?}"
    );
}
