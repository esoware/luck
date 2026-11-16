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
/// on one line, with each table in parentheses or bare.
fn broken_table_calls(levels: usize, has_parentheses: bool) -> String {
    let (open, empty, close) =
        if has_parentheses { ("f({", "f({})", "})") } else { ("f {", "f {}", "}") };
    let innermost = levels.saturating_sub(1);
    let mut output = format!("return {open}\n");
    for depth in 1..innermost {
        output.push_str(&"\t".repeat(depth));
        output.push_str(open);
        output.push('\n');
    }
    output.push_str(&"\t".repeat(innermost));
    output.push_str(empty);
    output.push_str(",\n");
    for depth in (1..innermost).rev() {
        output.push_str(&"\t".repeat(depth));
        output.push_str(close);
        output.push_str(",\n");
    }
    output.push_str(close);
    output.push('\n');
    output
}

#[test]
fn call_parentheses_never_format_valid_input_past_the_nesting_cap() {
    // `f{...}` gains parentheses by default, which make a call a level for its
    // argument and one for the field it holds, so past half of Lua 5.4's
    // limit every table stays bare.
    for (levels, has_parentheses) in [(50, true), (99, true), (100, false), (199, false)] {
        let expected = broken_table_calls(levels, has_parentheses);
        assert_format(&nested_table_calls(levels), &expected);
    }
}
