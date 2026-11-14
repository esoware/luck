use luck_formatter::{FormatOptions, IndentStyle};
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with};

#[test]
fn if_stmt() {
    assert_format(
        "\
if x then
y=1
elseif z then
y=2
else
y=3
end
",
        "\
if x then
\ty = 1
elseif z then
\ty = 2
else
\ty = 3
end
",
    );
}

#[test]
fn while_loop() {
    assert_format(
        "\
while x do
y=1
end
",
        "\
while x do
\ty = 1
end
",
    );
}

#[test]
fn repeat_until() {
    assert_format(
        "\
repeat
x=1
until done
",
        "\
repeat
\tx = 1
until done
",
    );
}

#[test]
fn numeric_for() {
    assert_format(
        "\
for i=1,10 do
x=i
end
",
        "\
for i = 1, 10 do
\tx = i
end
",
    );
}

#[test]
fn numeric_for_with_step() {
    assert_format(
        "\
for i=1,10,2 do
x=i
end
",
        "\
for i = 1, 10, 2 do
\tx = i
end
",
    );
}

#[test]
fn generic_for() {
    assert_format(
        "\
for k,v in pairs(t) do
x=v
end
",
        "\
for k, v in pairs(t) do
\tx = v
end
",
    );
}

#[test]
fn do_block() {
    assert_format(
        "\
do
x=1
end
",
        "\
do
\tx = 1
end
",
    );
}

#[test]
fn goto_and_label() {
    assert_format_with(
        "\
goto done
::done::
",
        "\
goto done
::done::
",
        LuaVersion::Lua52,
        &FormatOptions::default(),
    );
}

#[test]
fn semicolons_stripped() {
    assert_format("local x = 1;\n", "local x = 1\n");
}

#[test]
fn single_newline_between_stmts_stays_single() {
    assert_format(
        "\
local a = 1
local b = 2",
        "\
local a = 1
local b = 2
",
    );
}

#[test]
fn semicolon_runs_dropped() {
    assert_format("local a = 1;;;", "local a = 1\n");
}

#[test]
fn paren_stmt_after_a_call_keeps_its_semicolon_guard() {
    // A `(`-starting statement must not glue onto the previous call.
    // The `;` in the input is what makes these parse as two statements;
    // without it the parser reads one chained call.
    assert_format(
        "f();(g)()",
        "\
f()
;(g)()
",
    );
}

#[test]
fn empty_stmt_stripped() {
    assert_format_with(
        "\
local x = 1
;
",
        "local x = 1\n",
        LuaVersion::Lua52,
        &FormatOptions::default(),
    );
}

#[test]
fn multi_return() {
    assert_format("return a,b,c\n", "return a, b, c\n");
}

#[test]
fn lua54_attributes() {
    assert_format("local x <const> =5\n", "local x <const> = 5\n");
}

#[test]
fn spaces_indent() {
    let options = FormatOptions {
        indent_style: IndentStyle::Spaces,
        indent_width: 2,
        ..FormatOptions::default()
    };
    assert_format_with(
        "\
if x then
y=1
end
",
        "\
if x then
  y = 1
end
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn luau_compound_assignment() {
    assert_format_with("x+=1\n", "x += 1\n", LuaVersion::Luau, &FormatOptions::default());
}

#[test]
fn luau_type_declaration() {
    assert_format_with(
        "type Foo = number\n",
        "type Foo = number\n",
        LuaVersion::Luau,
        &FormatOptions::default(),
    );
}

#[test]
fn luau_export_type() {
    assert_format_with(
        "export type Foo = number\n",
        "export type Foo = number\n",
        LuaVersion::Luau,
        &FormatOptions::default(),
    );
}

#[test]
fn long_assignment_values_break() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "local a, b = very_long_value_1, very_long_value_2\n",
        "\
local a, b = very_long_value_1,
\tvery_long_value_2
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn long_if_condition_breaks() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "\
if very_long_condition_name then
x = 1
end
",
        "\
if
\tvery_long_condition_name
then
\tx = 1
end
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn long_condition_chain_breaks_level_with_its_first_operand() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "\
if first_condition and second_condition and third then
x = 1
elseif (other_condition or another_condition) then
x = 2
end
",
        "\
if
\tfirst_condition
\tand second_condition
\tand third
then
\tx = 1
elseif
\tother_condition
\tor another_condition
then
\tx = 2
end
",
        LuaVersion::Lua54,
        &options,
    );
    assert_format_with(
        "\
while first_condition and second_condition do
step()
end
",
        "\
while
\tfirst_condition
\tand second_condition
do
\tstep()
end
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn condition_breaks_between_comparisons_not_inside_them() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "\
if phase == Phase.ShouldUpdate or phase == Phase.WillUpdate or phase == Phase.Render then
x = 1
end
",
        "\
if
\tphase == Phase.ShouldUpdate
\tor phase == Phase.WillUpdate
\tor phase == Phase.Render
then
\tx = 1
end
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn chain_nested_in_a_condition_indents_under_its_operator() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "\
if alpha_condition or beta_condition and gamma_condition and delta_condition then
x = 1
end
",
        "\
if
\talpha_condition
\tor beta_condition
\t\tand gamma_condition
\t\tand delta_condition
then
\tx = 1
end
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn long_until_condition_chain_stays_indented() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "\
repeat
step()
until first_condition and second_condition
",
        "\
repeat
\tstep()
until first_condition
\tand second_condition
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn global_declaration_with_initializer() {
    // Lua 5.5: `global attnamelist ['=' explist]`.
    assert_format_with(
        "global x=1\n",
        "global x = 1\n",
        LuaVersion::Lua55,
        &FormatOptions::default(),
    );
    assert_format_with(
        "global a,b=1,f(2)\n",
        "global a, b = 1, f(2)\n",
        LuaVersion::Lua55,
        &FormatOptions::default(),
    );
}

#[test]
fn leading_attribute_formats_as_distributed_trailing() {
    // 5.5 manual section 3.3.7: a leading attribute applies to every name,
    // and the formatter rewrites it into the per-name trailing form.
    assert_format_with(
        "local <const> x, y = 1, 2\n",
        "local x <const>, y <const> = 1, 2\n",
        LuaVersion::Lua55,
        &FormatOptions::default(),
    );
}

#[test]
fn function_body_of_only_semicolons_formats_like_an_empty_body() {
    assert_format("function f() ; end\n", "function f() end\n");
}

#[test]
fn empty_do_block_has_no_blank_line() {
    assert_format(
        "do end\n",
        "\
do
end
",
    );
}

fn narrow() -> FormatOptions {
    FormatOptions { line_width: 30, ..FormatOptions::default() }
}

#[test]
fn assigned_values_move_to_the_next_line_when_only_it_fits_them() {
    for (source, expected, version) in [
        (
            "local alpha, beta = gamma + delta, 1\n",
            "\
local alpha, beta =
\tgamma + delta, 1
",
            LuaVersion::Lua54,
        ),
        (
            "alpha.beta.gamma = (x - 1) / 99\n",
            "\
alpha.beta.gamma =
\t(x - 1) / 99
",
            LuaVersion::Lua54,
        ),
        (
            "global alpha_value = (x - 1) / 99\n",
            "\
global alpha_value =
\t(x - 1) / 99
",
            LuaVersion::Lua55,
        ),
        (
            "counter_value_x += (x - 1) / 99\n",
            "\
counter_value_x +=
\t(x - 1) / 99
",
            LuaVersion::Luau,
        ),
    ] {
        assert_format_with(source, expected, version, &narrow());
    }
}

#[test]
fn assigned_values_that_fit_neither_line_hang_from_the_equals() {
    assert_format_with(
        "local alpha = first_operand_name + second_operand_name\n",
        "\
local alpha = first_operand_name
\t+ second_operand_name
",
        LuaVersion::Lua54,
        &narrow(),
    );
}

#[test]
fn a_lone_table_function_or_call_stays_on_the_equals_line() {
    for (source, expected) in [
        (
            "local value = { first = 1, second = 2 }\n",
            "\
local value = {
\tfirst = 1,
\tsecond = 2,
}
",
        ),
        (
            "local value = function() return 1 end\n",
            "\
local value = function()
\treturn 1
end
",
        ),
        (
            "local value = call_something(first, second)\n",
            "\
local value = call_something(
\tfirst,
\tsecond
)
",
        ),
    ] {
        assert_format_with(source, expected, LuaVersion::Lua54, &narrow());
    }
}
