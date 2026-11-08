//! Formatting known-tricky inputs twice, through `formatted`, which checks
//! that the output formats to itself and re-parses clean.

use crate::common::formatted;

#[test]
fn realistic_module() {
    let input = r#"local M={}
function M.init(config)
if not config then
config={}
end
M.name=config.name or "default"
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
"#;
    formatted(input);
}

#[test]
fn nested_control_flow() {
    let input = "for i=1,10 do
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
    formatted(input);
}

#[test]
fn complex_exprs() {
    let input = r#"local result=a+b*c-d/e%f
local s="hello ".."world"..tostring(42)
local t={a=1,b={c=2,d={e=3}},f=function() return true end}
"#;
    formatted(input);
}

#[test]
fn mixed_with_comments() {
    let input = "-- Module header
local M = {} -- the module

-- Initialize
function M.init()
    -- setup
    M.ready = true
end

return M -- done
";
    formatted(input);
}

#[test]
fn comment_inside_stmt_then_blank_line() {
    // No emitter claims a comment between a callee and its argument list, so
    // the call goes out verbatim, and the blank line after it has to survive
    // the second pass.
    formatted("goto l x\n--\n(\"\")\n\nc = x\n");
}

#[test]
fn empty_constructs() {
    let input = "local t = {}
function noop() end
do end
if true then end
";
    formatted(input);
}
