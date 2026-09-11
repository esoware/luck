use luck_formatter::{FormatOptions, format};
use luck_token::LuaVersion;

fn assert_idempotent(source: &str) {
    let result = format(source, LuaVersion::Lua54, &FormatOptions::default());
    assert!(result.errors.is_empty());
    let result2 = format(&result.output, LuaVersion::Lua54, &FormatOptions::default());
    assert_eq!(result.output, result2.output, "not idempotent");
    let reparse = luck_parser::parse(&result.output, LuaVersion::Lua54);
    assert!(
        reparse.errors.is_empty(),
        "re-parse failed: {:?}",
        reparse.errors
    );
}

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
    assert_idempotent(input);
}

#[test]
fn nested_control_flow() {
    let input = r#"for i=1,10 do
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
"#;
    assert_idempotent(input);
}

#[test]
fn complex_expressions() {
    let input = r#"local result=a+b*c-d/e%f
local s="hello ".."world"..tostring(42)
local t={a=1,b={c=2,d={e=3}},f=function() return true end}
"#;
    assert_idempotent(input);
}

#[test]
fn mixed_with_comments() {
    let input = r#"-- Module header
local M = {} -- the module

-- Initialize
function M.init()
    -- setup
    M.ready = true
end

return M -- done
"#;
    assert_idempotent(input);
}

#[test]
fn comment_inside_statement_then_blank_line() {
    // The comment sits between the callee and its argument list, so no
    // sub-emitter claims it and it lands on its own line after the statement.
    // Emitting the following blank as a statement gap would break
    // idempotency: a reparse reads the relocated comment as the next
    // statement's leading comment and drops the blank.
    assert_idempotent("goto l x\n--\n(\"\")\n\nc = x\n");
}

#[test]
fn empty_constructs() {
    let input = r#"local t = {}
function noop() end
do end
if true then end
"#;
    assert_idempotent(input);
}
