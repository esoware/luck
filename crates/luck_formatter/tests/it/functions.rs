use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

use crate::common::{assert_format, assert_format_with};

#[test]
fn function_def_flat() {
    assert_format(
        "\
function foo(a,b,c)
return a
end
",
        "\
function foo(a, b, c)
\treturn a
end
",
    );
}

#[test]
fn trailing_callback_is_hugged() {
    let source = "subscribe(event, function(value) consume(value) end)\n";
    let expected = "\
subscribe(event, function(value)
\tconsume(value)
end)
";
    for width in [60, 80, 120] {
        let options = FormatOptions { line_width: width, ..FormatOptions::default() };
        assert_format_with(source, expected, LuaVersion::Lua54, &options);
    }
}

#[test]
fn leading_callback_is_hugged_with_the_args_after_it() {
    assert_format(
        "Store.new(function(state) return state end, mockState, nil)\n",
        "\
Store.new(function(state)
\treturn state
end, mockState, nil)
",
    );
}

#[test]
fn nested_trailing_callbacks_are_hugged() {
    assert_format(
        "outer(1, function() inner(2, function() run() end) end)\n",
        "\
outer(1, function()
\tinner(2, function()
\t\trun()
\tend)
end)
",
    );
}

#[test]
fn trailing_table_is_hugged_when_the_call_is_too_long() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local frame = create(\"Frame\", {Size = size, Visible = true, Order = 3})\n",
        "\
local frame = create(\"Frame\", {
\tSize = size,
\tVisible = true,
\tOrder = 3,
})
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn leading_table_is_hugged_with_the_args_after_it() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local self = setmetatable({property = property, other = other}, MyClass)\n",
        "\
local self = setmetatable({
\tproperty = property,
\tother = other,
}, MyClass)
",
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
        "\
setmetatable({}, {
\t__index = function(self, key)
\t\treturn key
\tend,
})
",
    );
}

#[test]
fn several_block_args_are_hugged() {
    let element = "\
local element = createElement(\"Frame\", {
\tSize = size,
}, {
\tTitle = createElement(\"TextLabel\", {
\t\tText = title,
\t}),
})
";
    assert_format(element, element);
    assert_format(
        "foo({type = \"foo\",}, function(something) print(something) end)\n",
        "\
foo({
\ttype = \"foo\",
}, function(something)
\tprint(something)
end)
",
    );
    assert_format(
        "xpcall(function() risky() end, function(message) return message end)\n",
        "\
xpcall(function()
\trisky()
end, function(message)
\treturn message
end)
",
    );
}

#[test]
fn short_table_stays_flat_beside_a_hugged_callback() {
    assert_format(
        "foo({type = \"foo\"}, function(something) print(something) end)\n",
        "\
foo({ type = \"foo\" }, function(something)
\tprint(something)
end)
",
    );
}

#[test]
fn hugged_tables_all_break_when_the_call_is_too_long() {
    let options = FormatOptions { line_width: 40, ..FormatOptions::default() };
    assert_format_with(
        "local element = createElement(\"Frame\", {Size = size}, {Title = title, Body = body})\n",
        "\
local element = createElement(\"Frame\", {
\tSize = size,
}, {
\tTitle = title,
\tBody = body,
})
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn block_args_split_by_another_arg_expand_one_per_line() {
    assert_format(
        "doSomething({key = value,}, notATable, {key = other,})\n",
        "\
doSomething(
\t{
\t\tkey = value,
\t},
\tnotATable,
\t{
\t\tkey = other,
\t}
)
",
    );
}

#[test]
fn hug_gives_way_when_the_opening_line_is_too_long() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "subscribe(some_long_event_name, another_argument, function(value) consume(value) end)\n",
        "\
subscribe(
\tsome_long_event_name,
\tanother_argument,
\tfunction(value)
\t\tconsume(value)
\tend
)
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn hug_gives_way_when_the_closing_line_is_too_long() {
    let options = FormatOptions { line_width: 30, ..FormatOptions::default() };
    assert_format_with(
        "run(function() step() end, some_long_argument_name, another_one)\n",
        "\
run(
\tfunction()
\t\tstep()
\tend,
\tsome_long_argument_name,
\tanother_one
)
",
        LuaVersion::Lua54,
        &options,
    );
}

#[test]
fn hugged_callback_with_a_comment_in_the_list_expands() {
    assert_format(
        "\
subscribe(event, -- why
function(value) consume(value) end)
",
        "\
subscribe(
\tevent, -- why
\tfunction(value)
\t\tconsume(value)
\tend
)
",
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
        "\
local function foo(x)
return x
end
",
        "\
local function foo(x)
\treturn x
end
",
    );
}

#[test]
fn anonymous_function() {
    assert_format(
        "\
local f=function(x)
return x
end
",
        "\
local f = function(x)
\treturn x
end
",
    );
}

#[test]
fn method_chain_expanded() {
    let options = FormatOptions { line_width: 20, ..FormatOptions::default() };
    assert_format_with(
        "foo:bar():baz():qux()\n",
        "\
foo
\t:bar()
\t:baz()
\t:qux()
",
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
    assert_format(
        "\
function Consumer:render()
end
",
        "function Consumer:render() end\n",
    );
    assert_format(
        "\
local spy = createSpy(function()
end)
",
        "local spy = createSpy(function() end)\n",
    );
}

#[test]
fn function_body_of_only_a_comment_stays_open() {
    let source = "\
local f = function()
\t-- nothing yet
end
";
    assert_format(source, source);
}

#[test]
fn field_chain_after_a_callback_stays_on_the_closing_line() {
    let source = "\
expect(function()
\trun()
end).never.to.throw()
";
    assert_format(source, source);
}

#[test]
fn long_field_chain_after_a_callback_breaks_per_segment() {
    let options = FormatOptions { line_width: 15, ..FormatOptions::default() };
    assert_format_with(
        "expect(function() run() end).never.to.throw()\n",
        "\
expect(function()
\trun()
end)
\t.never
\t.to
\t.throw()
",
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
        "\
foo({
\tvery_long_key = 1,
\tanother_key = 2,
})
",
        LuaVersion::Lua54,
        &options,
    );
}
