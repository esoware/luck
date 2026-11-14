use core::fmt::Write as _;

use luck_formatter::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, HexCase,
    SpaceAfterFunction,
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
fn hex_prefix_lowered_while_digits_uppered() {
    let options = FormatOptions { hexadecimal_case: HexCase::Upper, ..FormatOptions::default() };
    assert_format_with(
        "local x = 0Xb0 + 1e5\n",
        "local x = 0xB0 + 1e5\n",
        LuaVersion::Luau,
        &options,
    );
}

#[test]
fn block_gaps_never_strips_leading_blank() {
    // The default, `Never`, strips blank lines at the top of a `do` body.
    let input = "\
do

\tlocal x = 1
end
";
    let expected = "\
do
\tlocal x = 1
end
";
    assert_format(input, expected);
}

#[test]
fn block_gaps_preserve_keeps_leading_blank() {
    let options = FormatOptions {
        block_newline_gaps: BlockNewlineGaps::Preserve,
        ..FormatOptions::default()
    };
    let input = "\
do

\tlocal x = 1
end
";
    let expected = "\
do

\tlocal x = 1
end
";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_keeps_trailing_blank() {
    let options = FormatOptions {
        block_newline_gaps: BlockNewlineGaps::Preserve,
        ..FormatOptions::default()
    };
    let input = "\
do
\tlocal x = 1

end
";
    let expected = "\
do
\tlocal x = 1

end
";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_function_body() {
    let options = FormatOptions {
        block_newline_gaps: BlockNewlineGaps::Preserve,
        ..FormatOptions::default()
    };
    let input = "\
local function f()

\tlocal x = 1

end
";
    let expected = "\
local function f()

\tlocal x = 1

end
";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn block_gaps_preserve_then_branch() {
    let options = FormatOptions {
        block_newline_gaps: BlockNewlineGaps::Preserve,
        ..FormatOptions::default()
    };
    let input = "\
if x then

\tlocal y = 1
end
";
    let expected = "\
if x then

\tlocal y = 1
end
";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_on_by_default() {
    assert_format(
        "\
local zeta = require(\"zeta\")
local alpha = require(\"alpha\")
",
        "\
local alpha = require(\"alpha\")
local zeta = require(\"zeta\")
",
    );
}

#[test]
fn sort_requires_off_keeps_the_source_order() {
    let options = FormatOptions { sort_requires: false, ..FormatOptions::default() };
    let input = "\
local zeta = require(\"zeta\")
local alpha = require(\"alpha\")
";
    assert_format_with(input, input, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_orders_instance_path_requires() {
    assert_format_with(
        "\
local Zebra = require(script.Parent.Zebra)
local Apple = require(Packages.Apple)
",
        "\
local Apple = require(Packages.Apple)
local Zebra = require(script.Parent.Zebra)
",
        LuaVersion::Luau,
        &FormatOptions::default(),
    );
}

#[test]
fn sort_requires_alphabetizes_consecutive() {
    let options = FormatOptions { sort_requires: true, ..FormatOptions::default() };
    let input = "\
local zeta = require(\"zeta\")
local alpha = require(\"alpha\")
local mid = require(\"mid\")
";
    let expected = "\
local alpha = require(\"alpha\")
local mid = require(\"mid\")
local zeta = require(\"zeta\")
";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_respects_blank_line_break() {
    let options = FormatOptions { sort_requires: true, ..FormatOptions::default() };
    // A blank line splits the run into two single requires, so nothing is
    // reordered.
    let input = "\
local zeta = require(\"zeta\")

local alpha = require(\"alpha\")
";
    let expected = "\
local zeta = require(\"zeta\")

local alpha = require(\"alpha\")
";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn sort_requires_keeps_a_header_comment_in_place() {
    assert_format(
        "\
-- a header
local zeta = require(\"zeta\")
local alpha = require(\"alpha\")
return alpha, zeta
",
        "\
-- a header
local alpha = require(\"alpha\")
local zeta = require(\"zeta\")
return alpha, zeta
",
    );
}

#[test]
fn sort_requires_format_off_excluded() {
    let input = "\
-- luck: format off
local zeta = require(\"zeta\")
local alpha = require(\"alpha\")
-- luck: format on
";
    assert_format(input, input);
}

#[test]
fn sort_requires_run_ends_at_another_stmt() {
    for input in [
        "\
local zeta = require(\"zeta\")
local x = 1
local alpha = require(\"alpha\")
",
        "\
local zeta = require(\"zeta\")
local a, b = 1, 2
local alpha = require(\"alpha\")
",
    ] {
        assert_format(input, input);
    }
}

#[test]
fn sort_requires_keeps_requires_that_may_have_side_effects() {
    let input = "\
local zeta = require(\"zeta\")()
local alpha = require(\"alpha\")()
";
    assert_format(input, input);
}

#[test]
fn sort_requires_orders_get_service_with_requires() {
    assert_format(
        "\
local zeta = require(\"zeta\")
local alpha = game:GetService(\"Players\")
",
        "\
local alpha = game:GetService(\"Players\")
local zeta = require(\"zeta\")
",
    );
}

#[test]
fn sort_requires_orders_requires_of_field_paths() {
    assert_format(
        "\
local Zebra = require(script.Parent.Zebra)
local Apple = require(Packages.Apple)
local Mango = require(module)
",
        "\
local Apple = require(Packages.Apple)
local Mango = require(module)
local Zebra = require(script.Parent.Zebra)
",
    );
}

#[test]
fn sort_requires_keeps_a_require_of_a_path_rooted_in_the_run() {
    let input = "\
local Packages = require(script.Packages)
local Apple = require(Packages.Apple)
";
    assert_format(input, input);
}

#[test]
fn sort_requires_keeps_a_require_of_a_computed_path() {
    for input in [
        "\
local Zebra = require(script:FindFirstChild(\"Zebra\"))
local Apple = require(script.Apple)
",
        "\
local Zebra = require(script[name])
local Apple = require(script.Apple)
",
    ] {
        assert_format(input, input);
    }
}

#[test]
fn sort_requires_orders_a_run_in_a_function_body() {
    let input = "\
local function f()
\tlocal zeta = require(\"zeta\")
\tlocal alpha = require(\"alpha\")
end
";
    let expected = "\
local function f()
\tlocal alpha = require(\"alpha\")
\tlocal zeta = require(\"zeta\")
end
";
    assert_format(input, expected);
}

#[test]
fn sort_requires_moves_trailing_comments_with_their_stmts() {
    assert_format(
        "\
local zeta = require(\"zeta\") -- last
local alpha = require(\"alpha\") --[[ first ]]
",
        "\
local alpha = require(\"alpha\") --[[ first ]]
local zeta = require(\"zeta\") -- last
",
    );
}

#[test]
fn sort_requires_orders_stmts_sharing_a_source_line_as_the_lines_they_print() {
    assert_format(
        "local zeta = require(\"zeta\") local alpha = require(\"alpha\")\n",
        "\
local alpha = require(\"alpha\")
local zeta = require(\"zeta\")
",
    );
    assert_format(
        "\
local zeta = require(\"zeta\") -- z
local beta = require(\"beta\") local alpha = require(\"alpha\") --[[ a ]]
do
\tlocal y = require(\"y\");local x = require(\"x\")
end
",
        "\
local alpha = require(\"alpha\") --[[ a ]]
local beta = require(\"beta\")
local zeta = require(\"zeta\") -- z
do
\tlocal x = require(\"x\")
\tlocal y = require(\"y\")
end
",
    );
}

#[test]
fn space_after_function_never_default() {
    assert_format_with(
        "local function f() return 1 end\n",
        "\
local function f()
\treturn 1
end
",
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
        "\
local function f() return 1 end
f()
",
        "\
local function f ()
\treturn 1
end
f()
",
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
        "\
local function f() return 1 end
f()
",
        "\
local function f()
\treturn 1
end
f ()
",
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
        "\
local function f() return 1 end
f()
",
        "\
local function f ()
\treturn 1
end
f ()
",
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
    assert_format(
        "\
-- luck: format ignore
local   x   =   1
local   y   =   2
",
        "\
-- luck: format ignore
local   x   =   1
local y = 2
",
    );
}

#[test]
fn magic_trailing_comma_off_keeps_packed_table() {
    let options = FormatOptions { magic_trailing_comma: false, ..FormatOptions::default() };
    let input = "local t = { 1, 2, 3, }\n";
    assert_format_with(input, "local t = { 1, 2, 3 }\n", LuaVersion::Lua54, &options);
}

#[test]
fn magic_trailing_comma_on_expands_table() {
    // The expanded output keeps its trailing comma, so the idempotency check
    // finds the second pass multi-line too.
    let options = FormatOptions { magic_trailing_comma: true, ..FormatOptions::default() };
    let input = "local t = { 1, 2, 3, }\n";
    let expected = "\
local t = {
\t1,
\t2,
\t3,
}
";
    assert_format_with(input, expected, LuaVersion::Lua54, &options);
}

#[test]
fn magic_trailing_comma_keeps_a_broken_table_broken_by_default() {
    let input = "\
local t = {
\ttype = \"baz\",
}
";
    assert_format(input, input);
}

#[test]
fn magic_trailing_comma_keeps_a_packed_list_packed() {
    // The values stay several to a line, repacked to the width, and the
    // table stays broken.
    assert_format(
        "\
local libs = {
\t\"roact\", \"rodux\",
\t\"cryo\",
}
",
        "\
local libs = {
\t\"roact\", \"rodux\", \"cryo\",
}
",
    );
}

#[test]
fn magic_trailing_comma_gives_a_wrapped_fill_its_comma() {
    let options = FormatOptions { line_width: 12, ..FormatOptions::default() };
    assert_format_with(
        "local t = {1,2,3,4,5,6}\n",
        "\
local t = {
\t1, 2, 3,
\t4, 5, 6,
}
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn magic_trailing_comma_on_expands_call_args_via_table() {
    // No dialect allows a trailing comma after the last argument, so the
    // option can only reach a call through a table argument.
    let options = FormatOptions { magic_trailing_comma: true, ..FormatOptions::default() };
    assert_format_with(
        "f({ 1, 2, 3, })\n",
        "\
f({
\t1,
\t2,
\t3,
})
",
        LuaVersion::Lua54,
        &options,
    );
}

fn preserving_gaps_options() -> FormatOptions {
    FormatOptions { block_newline_gaps: BlockNewlineGaps::Preserve, ..FormatOptions::default() }
}

#[test]
fn sort_requires_keeps_a_stmt_before_a_comment_at_eof() {
    // A last line with no newline must not swallow the next sorted line into
    // its comment.
    assert_format(
        "\
local z = require(\"z\")
local a = require(\"a\") -- note",
        "\
local a = require(\"a\") -- note
local z = require(\"z\")
",
    );
}

#[test]
fn sort_requires_keeps_a_comment_between_requires() {
    assert_format(
        "\
local b = require(\"b\")
; -- important
local a = require(\"a\")
",
        "\
local b = require(\"b\")
-- important
local a = require(\"a\")
",
    );
}

#[test]
fn sort_requires_keeps_a_multi_line_comment_between_requires() {
    assert_format(
        "\
local z = require(\"z\") --[[
]] local a = require(\"a\")
",
        "\
local z = require(\"z\") --[[
]]
local a = require(\"a\")
",
    );
}

#[test]
fn sort_requires_keeps_a_require_inside_the_block_opened_on_its_line() {
    assert_format(
        "\
do local z = require(\"z\")
local a = require(\"a\")
end
",
        "\
do
\tlocal a = require(\"a\")
\tlocal z = require(\"z\")
end
",
    );
}

#[test]
fn sort_requires_keeps_the_end_closing_on_a_require_line() {
    assert_format(
        "\
if x then
local z = require(\"z\")
local a = require(\"a\") end
",
        "\
if x then
\tlocal a = require(\"a\")
\tlocal z = require(\"z\")
end
",
    );
}

#[test]
fn sort_requires_keeps_a_binding_before_the_get_service_that_uses_it() {
    let input = "\
local zeta = require(\"z\")
local alpha = zeta:GetService(\"x\")
";
    assert_format(input, input);
}

#[test]
fn sort_requires_keeps_a_shadowing_require_before_its_callers() {
    let input = "\
local require = require(\"custom\")
local a = require(\"a\")
";
    assert_format(input, input);
}

#[test]
fn sort_requires_keeps_an_ignored_stmt_under_its_directive() {
    let input = "\
-- luck: ignore
local   z  = require(\"z\")
local a = require(\"a\")
";
    assert_format(input, input);
}

#[test]
fn sort_requires_keeps_the_close_order_of_to_be_closed_variables() {
    // To-be-closed variables close in reverse declaration order.
    let input = "\
local z <close> = require(\"z\")
local a <close> = require(\"a\")
";
    assert_format(input, input);
}

#[test]
fn magic_trailing_comma_formats_a_wrapped_fill_idempotently() {
    // The comma a width-wrapped fill adds must not read as a magic trailing
    // comma on the next pass.
    let options =
        FormatOptions { magic_trailing_comma: true, line_width: 12, ..FormatOptions::default() };
    assert_format_with(
        "local t = {1,2,3,4,5,6,7,8,9,10}\n",
        "\
local t = {
\t1, 2, 3,
\t4, 5, 6,
\t7, 8, 9,
\t10,
}
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn block_gaps_preserve_keeps_a_blank_after_a_trailing_comment() {
    let input = "\
do
\tx() -- c

end
";
    assert_format_with(input, input, LuaVersion::Lua54, &preserving_gaps_options());
}

#[test]
fn block_gaps_preserve_keeps_a_blank_after_a_semicolon() {
    assert_format_with(
        "\
do
\tx();

end
",
        "\
do
\tx()

end
",
        LuaVersion::Lua54,
        &preserving_gaps_options(),
    );
}

#[test]
fn collapse_with_preserved_gaps_keeps_a_valid_layout() {
    let options = FormatOptions {
        collapse_simple_statement: CollapseSimpleStatement::ConditionalOnly,
        ..preserving_gaps_options()
    };
    let input = "\
if a then

\tx = 1

end
";
    assert_format_with(input, input, LuaVersion::Lua54, &options);
}

#[test]
fn a_long_run_of_requires_sorts_without_comparing_every_pair() {
    // Whether one line of a run reads another's binding is a lookup per line.
    // Checked pair by pair, this takes minutes.
    let count = 180_000usize;
    let require_line = |index: usize| {
        let module = index.wrapping_mul(7919).rem_euclid(count);
        format!("local m{index} = require(\"mod{module}\")\n")
    };
    let mut by_name: Vec<usize> = (0..count).collect();
    by_name.sort_by_key(usize::to_string);
    assert_format(
        &(0..count).map(require_line).collect::<String>(),
        &by_name.into_iter().map(require_line).collect::<String>(),
    );
}

#[test]
fn a_long_string_heading_a_long_chain_formats_without_remeasuring_it() {
    // Each `:` run opens a group at the string. Measured again by every group,
    // the string takes minutes.
    let head = format!("local x = (\"{}\")", "a".repeat(4_000_000));
    assert_format(
        &format!("{head}{}\n", ":b():c().d".repeat(8_000)),
        &format!(
            "{head}{}\n",
            "
\t:b()
\t:c().d"
                .repeat(8_000)
        ),
    );
}

#[test]
fn many_format_off_regions_format_without_rescanning_the_regions() {
    // Each statement asks which region comes next. Answered by walking the
    // regions from the first, this takes minutes.
    let mut source = String::new();
    let mut expected = String::new();
    for index in 0..140_000u32 {
        let region = format!(
            "\
-- luck: format off
x{index}  =  {index}
-- luck: format on
"
        );
        writeln!(source, "{region}y{index}   =   {index}").unwrap();
        writeln!(expected, "{region}y{index} = {index}").unwrap();
    }
    assert_format(&source, &expected);
}
