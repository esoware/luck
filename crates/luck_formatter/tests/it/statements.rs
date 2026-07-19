use crate::common::{assert_format, assert_format_with};
use luck_formatter::{FormatOptions, IndentStyle};
use luck_token::LuaVersion;

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
    assert_format(
        "for i=1,10 do\nx=i\nend\n",
        "for i = 1, 10 do\n\tx = i\nend\n",
    );
}

#[test]
fn numeric_for_with_step() {
    assert_format(
        "for i=1,10,2 do\nx=i\nend\n",
        "for i = 1, 10, 2 do\n\tx = i\nend\n",
    );
}

#[test]
fn generic_for() {
    assert_format(
        "for k,v in pairs(t) do\nx=v\nend\n",
        "for k, v in pairs(t) do\n\tx = v\nend\n",
    );
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
    assert_format_with(
        "x+=1\n",
        "x += 1\n",
        LuaVersion::Luau,
        &FormatOptions::default(),
    );
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
    let opts = FormatOptions {
        line_width: 30,
        ..FormatOptions::default()
    };
    assert_format_with(
        "local a, b = very_long_value_1, very_long_value_2\n",
        "local a, b = very_long_value_1,\n\tvery_long_value_2\n",
        LuaVersion::Lua54,
        &opts,
    );
}

#[test]
fn long_if_condition_breaks() {
    let opts = FormatOptions {
        line_width: 30,
        ..FormatOptions::default()
    };
    assert_format_with(
        "if very_long_condition_name then\nx = 1\nend\n",
        "if\n\tvery_long_condition_name\nthen\n\tx = 1\nend\n",
        LuaVersion::Lua54,
        &opts,
    );
}
