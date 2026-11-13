use luck_formatter::{FormatOptions, IndentStyle};
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with, format_once, formatted, formatted_with};

#[test]
fn if_stmt() {
    assert_format(
        "if x then\ny=1\nelseif z then\ny=2\nelse\ny=3\nend\n",
        "if x then\n\ty = 1\nelseif z then\n\ty = 2\nelse\n\ty = 3\nend\n",
    );
}

#[test]
fn while_loop() {
    assert_format("while x do\ny=1\nend\n", "while x do\n\ty = 1\nend\n");
}

#[test]
fn repeat_until() {
    assert_format("repeat\nx=1\nuntil done\n", "repeat\n\tx = 1\nuntil done\n");
}

#[test]
fn numeric_for() {
    assert_format("for i=1,10 do\nx=i\nend\n", "for i = 1, 10 do\n\tx = i\nend\n");
}

#[test]
fn numeric_for_with_step() {
    assert_format("for i=1,10,2 do\nx=i\nend\n", "for i = 1, 10, 2 do\n\tx = i\nend\n");
}

#[test]
fn generic_for() {
    assert_format("for k,v in pairs(t) do\nx=v\nend\n", "for k, v in pairs(t) do\n\tx = v\nend\n");
}

#[test]
fn do_block() {
    assert_format("do\nx=1\nend\n", "do\n\tx = 1\nend\n");
}

#[test]
fn goto_and_label() {
    assert_format_with(
        "goto done\n::done::\n",
        "goto done\n::done::\n",
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
    assert_format("local a = 1\nlocal b = 2", "local a = 1\nlocal b = 2\n");
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
    let output = formatted("f();(g)()");
    assert!(output.contains(";("));
}

#[test]
fn empty_stmt_stripped() {
    assert_format_with(
        "local x = 1\n;\n",
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
        "if x then\ny=1\nend\n",
        "if x then\n  y = 1\nend\n",
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
        "local a, b = very_long_value_1,\n\tvery_long_value_2\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn long_if_condition_breaks() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "if very_long_condition_name then\nx = 1\nend\n",
        "if\n\tvery_long_condition_name\nthen\n\tx = 1\nend\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn long_condition_chain_breaks_level_with_its_first_operand() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "if first_condition and second_condition and third then\nx = 1\nelseif (other_condition or another_condition) then\nx = 2\nend\n",
        "if\n\tfirst_condition\n\tand second_condition\n\tand third\nthen\n\tx = 1\nelseif\n\tother_condition\n\tor another_condition\nthen\n\tx = 2\nend\n",
        LuaVersion::Lua54,
        &options,
    );
    assert_format_with(
        "while first_condition and second_condition do\nstep()\nend\n",
        "while\n\tfirst_condition\n\tand second_condition\ndo\n\tstep()\nend\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn condition_breaks_between_comparisons_not_inside_them() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "if phase == Phase.ShouldUpdate or phase == Phase.WillUpdate or phase == Phase.Render then\nx = 1\nend\n",
        "if\n\tphase == Phase.ShouldUpdate\n\tor phase == Phase.WillUpdate\n\tor phase == Phase.Render\nthen\n\tx = 1\nend\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn chain_nested_in_a_condition_indents_under_its_operator() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "if alpha_condition or beta_condition and gamma_condition and delta_condition then\nx = 1\nend\n",
        "if\n\talpha_condition\n\tor beta_condition\n\t\tand gamma_condition\n\t\tand delta_condition\nthen\n\tx = 1\nend\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn long_until_condition_chain_stays_indented() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "repeat\nstep()\nuntil first_condition and second_condition\n",
        "repeat\n\tstep()\nuntil first_condition\n\tand second_condition\n",
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
    assert_eq!(
        format_once("function f() ; end\n", LuaVersion::Lua54, &FormatOptions::default()),
        "function f() end\n",
        "a dropped `;` must leave the same output as an empty body"
    );
}

#[test]
fn empty_do_block_has_no_blank_line() {
    assert_eq!(
        format_once("do end\n", LuaVersion::Lua54, &FormatOptions::default()),
        "do\nend\n",
        "`block_newline_gaps = never` strips blank lines at the start and end of every block body"
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
            "local alpha, beta =\n\tgamma + delta, 1\n",
            LuaVersion::Lua54,
        ),
        (
            "alpha.beta.gamma = (x - 1) / 99\n",
            "alpha.beta.gamma =\n\t(x - 1) / 99\n",
            LuaVersion::Lua54,
        ),
        (
            "global alpha_value = (x - 1) / 99\n",
            "global alpha_value =\n\t(x - 1) / 99\n",
            LuaVersion::Lua55,
        ),
        (
            "counter_value_x += (x - 1) / 99\n",
            "counter_value_x +=\n\t(x - 1) / 99\n",
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
        "local alpha = first_operand_name\n\t+ second_operand_name\n",
        LuaVersion::Lua54,
        &narrow(),
    );
}

#[test]
fn a_lone_table_function_or_call_stays_on_the_equals_line() {
    for (source, first_line) in [
        ("local value = { first = 1, second = 2 }\n", "local value = {"),
        ("local value = function() return 1 end\n", "local value = function()"),
        ("local value = call_something(first, second)\n", "local value = call_something("),
    ] {
        let output = formatted_with(source, LuaVersion::Lua54, &narrow());
        assert_eq!(output.lines().next(), Some(first_line), "{output}");
    }
}
