use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

use crate::common::assert_format;

#[test]
fn realistic_module() {
    let input = "\
local M={}
function M.init(config)
if not config then
config={}
end
M.name=config.name or \"default\"
M.items={}
return M
end
function M:add(item)
table.insert(self.items,item)
end
function M:get_all()
return self.items
end
return M
";
    let expected = "\
local M = {}
function M.init(config)
\tif not config then
\t\tconfig = {}
\tend
\tM.name = config.name or \"default\"
\tM.items = {}
\treturn M
end
function M:add(item)
\ttable.insert(self.items, item)
end
function M:get_all()
\treturn self.items
end
return M
";
    assert_format(input, expected);
}

#[test]
fn nested_control_flow() {
    let input = "\
for i=1,10 do
if i%2==0 then
while true do
break
end
else
repeat
x=x+1
until x>10
end
end
";
    let expected = "\
for i = 1, 10 do
\tif i % 2 == 0 then
\t\twhile true do
\t\t\tbreak
\t\tend
\telse
\t\trepeat
\t\t\tx = x + 1
\t\tuntil x > 10
\tend
end
";
    assert_format(input, expected);
}

#[test]
fn complex_exprs() {
    let input = "\
local result=a+b*c-d/e%f
local s=\"hello \"..\"world\"..tostring(42)
local t={a=1,b={c=2,d={e=3}},f=function() return true end}
";
    let expected = "\
local result = a + b * c - d / e % f
local s = \"hello \" .. \"world\" .. tostring(42)
local t = {
\ta = 1,
\tb = { c = 2, d = { e = 3 } },
\tf = function()
\t\treturn true
\tend,
}
";
    assert_format(input, expected);
}

#[test]
fn mixed_with_comments() {
    let input = "\
-- Module header
local M = {} -- the module

-- Initialize
function M.init()
    -- setup
    M.ready = true
end

return M -- done
";
    let expected = "\
-- Module header
local M = {} -- the module

-- Initialize
function M.init()
\t-- setup
\tM.ready = true
end

return M -- done
";
    assert_format(input, expected);
}

#[test]
fn comment_inside_stmt_then_blank_line() {
    // No emitter claims a comment between a callee and its argument list, so
    // the call goes out verbatim, and the blank line after it has to survive
    // the second pass.
    assert_format(
        "\
goto l x
--
(\"\")

c = x
",
        "\
goto l
x
--
(\"\")

c = x
",
    );
}

#[test]
fn empty_constructs() {
    let input = "\
local t = {}
function noop() end
do end
if true then end
";
    let expected = "\
local t = {}
function noop() end
do
end
if true then
end
";
    assert_format(input, expected);
}

fn nested_table_calls(levels: usize) -> String {
    format!("return {}{}\n", "f{".repeat(levels), "}".repeat(levels))
}

/// [`nested_table_calls`] formatted, where no call is short enough to stay
/// on one line.
fn broken_table_calls(levels: usize) -> String {
    let innermost = levels.saturating_sub(1);
    let mut output = String::from("return f({\n");
    for depth in 1..innermost {
        output.push_str(&"\t".repeat(depth));
        output.push_str("f({\n");
    }
    output.push_str(&"\t".repeat(innermost));
    output.push_str("f({}),\n");
    for depth in (1..innermost).rev() {
        output.push_str(&"\t".repeat(depth));
        output.push_str("}),\n");
    }
    output.push_str("})\n");
    output
}

#[test]
fn call_parentheses_never_format_valid_input_past_the_nesting_cap() {
    // `f{...}` gains parentheses by default, so a bare table argument has to
    // count toward the cap as the parenthesized one it prints as does.
    for levels in [100, 126, 127] {
        assert_format(&nested_table_calls(levels), &broken_table_calls(levels));
    }
    for levels in [128, 129, 200, 255] {
        let source = nested_table_calls(levels);
        let result = luck_formatter::format(&source, LuaVersion::Lua54, &FormatOptions::default());
        let [error] = result.errors.as_slice() else {
            panic!("{levels} levels: {:?}", result.errors);
        };
        assert!(error.message.contains("maximum nesting depth exceeded"), "{levels}: {error:?}");
    }
}
