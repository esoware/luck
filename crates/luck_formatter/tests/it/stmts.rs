use luck_formatter::{FormatOptions, IndentStyle};
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with, formatted};

#[test]
fn if_statement() {
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
fn stmts_separated_by_newline() {
    let output = formatted("local a = 1\nlocal b = 2");
    assert_eq!(output.trim().lines().count(), 2);
}

#[test]
fn single_newline_stays_single() {
    let output = formatted("local a = 1\nlocal b = 2");
    assert!(!output.trim().contains("\n\n"));
}

#[test]
fn semicolon_runs_dropped() {
    let output = formatted("local a = 1;;;");
    assert_eq!(output.trim(), "local a = 1");
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
fn empty_statement_stripped() {
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
