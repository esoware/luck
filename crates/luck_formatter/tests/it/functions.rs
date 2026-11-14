use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with};

#[test]
fn function_def_flat() {
    assert_format(
        "function foo(a,b,c)\nreturn a\nend\n",
        "function foo(a, b, c)\n\treturn a\nend\n",
    );
}

#[test]
fn trailing_callback_is_hugged() {
    let source = "subscribe(event, function(value) consume(value) end)\n";
    let expected = "subscribe(event, function(value)\n\tconsume(value)\nend)\n";
    for width in [60, 80, 120] {
        let options = FormatOptions { line_width: width, ..FormatOptions::default() };
        assert_format_with(source, expected, LuaVersion::Lua54, &options);
    }
}

#[test]
fn leading_callback_is_hugged_with_the_args_after_it() {
    assert_format(
        "Store.new(function(state) return state end, mockState, nil)\n",
        "Store.new(function(state)\n\treturn state\nend, mockState, nil)\n",
    );
}

#[test]
fn nested_trailing_callbacks_are_hugged() {
    assert_format(
        "outer(1, function() inner(2, function() run() end) end)\n",
        "outer(1, function()\n\tinner(2, function()\n\t\trun()\n\tend)\nend)\n",
    );
}

#[test]
fn trailing_table_is_hugged_when_the_call_is_too_long() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local frame = create(\"Frame\", {Size = size, Visible = true, Order = 3})\n",
        "local frame = create(\"Frame\", {\n\tSize = size,\n\tVisible = true,\n\tOrder = 3,\n})\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn leading_table_is_hugged_with_the_args_after_it() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local self = setmetatable({property = property, other = other}, MyClass)\n",
        "local self = setmetatable({\n\tproperty = property,\n\tother = other,\n}, MyClass)\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn hugged_table_stays_flat_when_the_call_fits() {
    assert_format("create(\"Frame\", {Size = size})\n", "create(\"Frame\", { Size = size })\n");
}

#[test]
fn empty_table_arg_does_not_stop_a_hug() {
    assert_format(
        "setmetatable({}, {__index = function(self, key) return key end})\n",
        "setmetatable({}, {\n\t__index = function(self, key)\n\t\treturn key\n\tend,\n})\n",
    );
}

#[test]
fn several_block_args_are_hugged() {
    let element = "local element = createElement(\"Frame\", {\n\tSize = size,\n}, {\n\tTitle = createElement(\"TextLabel\", {\n\t\tText = title,\n\t}),\n})\n";
    assert_format(element, element);
    assert_format(
        "foo({type = \"foo\",}, function(something) print(something) end)\n",
        "foo({\n\ttype = \"foo\",\n}, function(something)\n\tprint(something)\nend)\n",
    );
    assert_format(
        "xpcall(function() risky() end, function(message) return message end)\n",
        "xpcall(function()\n\trisky()\nend, function(message)\n\treturn message\nend)\n",
    );
}

#[test]
fn short_table_stays_flat_beside_a_hugged_callback() {
    assert_format(
        "foo({type = \"foo\"}, function(something) print(something) end)\n",
        "foo({ type = \"foo\" }, function(something)\n\tprint(something)\nend)\n",
    );
}

#[test]
fn hugged_tables_all_break_when_the_call_is_too_long() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local element = createElement(\"Frame\", {Size = size}, {Title = title, Body = body})\n",
        "local element = createElement(\"Frame\", {\n\tSize = size,\n}, {\n\tTitle = title,\n\tBody = body,\n})\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn block_args_split_by_another_arg_expand_one_per_line() {
    assert_format(
        "doSomething({key = value,}, notATable, {key = other,})\n",
        "doSomething(\n\t{\n\t\tkey = value,\n\t},\n\tnotATable,\n\t{\n\t\tkey = other,\n\t}\n)\n",
    );
}

#[test]
fn hug_gives_way_when_the_opening_line_is_too_long() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "subscribe(some_long_event_name, another_argument, function(value) consume(value) end)\n",
        "subscribe(\n\tsome_long_event_name,\n\tanother_argument,\n\tfunction(value)\n\t\tconsume(value)\n\tend\n)\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn hug_gives_way_when_the_closing_line_is_too_long() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "run(function() step() end, some_long_argument_name, another_one)\n",
        "run(\n\tfunction()\n\t\tstep()\n\tend,\n\tsome_long_argument_name,\n\tanother_one\n)\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn hugged_callback_with_a_comment_in_the_list_expands() {
    assert_format(
        "subscribe(event, -- why\nfunction(value) consume(value) end)\n",
        "subscribe(\n\tevent, -- why\n\tfunction(value)\n\t\tconsume(value)\n\tend\n)\n",
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
    let options = FormatOptions { line_width: 20, ..FormatOptions::default() };
    assert_format_with(
        "foo:bar():baz():qux()\n",
        "foo\n\t:bar()\n\t:baz()\n\t:qux()\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn deeply_nested_calls() {
    assert_format("foo(bar(baz(x)))\n", "foo(bar(baz(x)))\n");
}

#[test]
fn empty_function_body_stays_on_the_header_line() {
    assert_format("local f=function()end\n", "local f = function() end\n");
    assert_format("function Consumer:render()\nend\n", "function Consumer:render() end\n");
    assert_format(
        "local spy = createSpy(function()\nend)\n",
        "local spy = createSpy(function() end)\n",
    );
}

#[test]
fn function_body_of_only_a_comment_stays_open() {
    let source = "local f = function()\n\t-- nothing yet\nend\n";
    assert_format(source, source);
}

#[test]
fn field_chain_after_a_callback_stays_on_the_closing_line() {
    let source = "expect(function()\n\trun()\nend).never.to.throw()\n";
    assert_format(source, source);
}

#[test]
fn long_field_chain_after_a_callback_breaks_per_segment() {
    let options = FormatOptions { line_width: 15, ..FormatOptions::default() };
    assert_format_with(
        "expect(function() run() end).never.to.throw()\n",
        "expect(function()\n\trun()\nend)\n\t.never\n\t.to\n\t.throw()\n",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn field_access_on_call_result() {
    // `foo().bar` isn't a statement, so it goes on the right-hand side.
    assert_format("local x = foo().bar\n", "local x = foo().bar\n");
}

#[test]
fn single_table_arg_hugged() {
    assert_format("foo({a=1,b=2})\n", "foo({ a = 1, b = 2 })\n");
}

#[test]
fn single_table_arg_hugged_multiline() {
    let options = FormatOptions { line_width: 20, ..FormatOptions::default() };
    assert_format_with(
        "foo({very_long_key=1,another_key=2})\n",
        "foo({\n\tvery_long_key = 1,\n\tanother_key = 2,\n})\n",
        LuaVersion::Lua54,
        &options,
    );
}
