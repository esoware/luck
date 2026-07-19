use crate::common::{assert_format, assert_format_with};
use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

#[test]
fn function_def_flat() {
    assert_format(
        "function foo(a,b,c)\nreturn a\nend\n",
        "function foo(a, b, c)\n\treturn a\nend\n",
    );
}

#[test]
fn function_call_flat() {
    assert_format("foo(a,b,c)\n", "foo(a, b, c)\n");
}

#[test]
fn bare_string_gets_parens() {
    assert_format("require \"module\"\n", "require(\"module\")\n");
}

#[test]
fn bare_table_gets_parens() {
    assert_format("foo {a=1}\n", "foo({ a = 1 })\n");
}

#[test]
fn method_call() {
    assert_format("foo:bar(x)\n", "foo:bar(x)\n");
}

#[test]
fn local_function() {
    assert_format(
        "local function foo(x)\nreturn x\nend\n",
        "local function foo(x)\n\treturn x\nend\n",
    );
}

#[test]
fn anonymous_function() {
    assert_format(
        "local f=function(x)\nreturn x\nend\n",
        "local f = function(x)\n\treturn x\nend\n",
    );
}

#[test]
fn method_chain_expanded() {
    let opts = FormatOptions {
        line_width: 20,
        ..FormatOptions::default()
    };
    assert_format_with(
        "foo:bar():baz():qux()\n",
        "foo\n\t:bar()\n\t:baz()\n\t:qux()\n",
        LuaVersion::Lua54,
        &opts,
    );
}

#[test]
fn deeply_nested_calls() {
    assert_format("foo(bar(baz(x)))\n", "foo(bar(baz(x)))\n");
}

#[test]
fn empty_function_body() {
    assert_format("local f=function()end\n", "local f = function()\nend\n");
}

#[test]
fn method_call_on_call_result() {
    // `foo().bar` is a field access expression, not a statement - test as rhs
    assert_format("local x = foo().bar\n", "local x = foo().bar\n");
}

#[test]
fn single_table_arg_hugged() {
    assert_format("foo({a=1,b=2})\n", "foo({ a = 1, b = 2 })\n");
}

#[test]
fn single_table_arg_hugged_multiline() {
    let opts = FormatOptions {
        line_width: 20,
        ..FormatOptions::default()
    };
    assert_format_with(
        "foo({very_long_key=1,another_key=2})\n",
        "foo({\n\tvery_long_key = 1,\n\tanother_key = 2,\n})\n",
        LuaVersion::Lua54,
        &opts,
    );
}
