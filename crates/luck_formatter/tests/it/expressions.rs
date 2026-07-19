use crate::common::{assert_format, assert_format_with};
use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

#[test]
fn literals() {
    assert_format("local x=1\n", "local x = 1\n");
    assert_format("local x=true\n", "local x = true\n");
    assert_format("local x=nil\n", "local x = nil\n");
    assert_format("local x = \"hello\"\n", "local x = \"hello\"\n");
}

#[test]
fn binary_op_flat() {
    assert_format("local x=a+b+c\n", "local x = a + b + c\n");
}

#[test]
fn unary_op() {
    assert_format("local x=-1\n", "local x = -1\n");
    assert_format("local x=not y\n", "local x = not y\n");
    assert_format("local x=#t\n", "local x = #t\n");
}

#[test]
fn parenthesized() {
    assert_format("local x=(a+b)\n", "local x = (a + b)\n");
}

#[test]
fn binary_op_expanded() {
    let options = FormatOptions {
        line_width: 20,
        ..FormatOptions::default()
    };
    assert_format_with(
        "local x=aaaa+bbbb+cccc\n",
        "local x = aaaa\n\t+ bbbb\n\t+ cccc\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn multi_assignment() {
    assert_format("x,y=1,2\n", "x, y = 1, 2\n");
}

#[test]
fn condition_parens_removed() {
    assert_format("if (x) then\ny = 1\nend\n", "if x then\n\ty = 1\nend\n");
}
