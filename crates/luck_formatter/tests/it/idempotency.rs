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

#[test]
fn call_parentheses_never_format_valid_input_past_the_nesting_cap() {
    // `f{...}` gains parentheses by default, so a bare table argument has to
    // count toward the cap as the parenthesized one it prints as does.
    let options = luck_formatter::FormatOptions::default();
    for nesting in [100usize, 126, 127, 128, 129, 200, 255] {
        let source = format!("return {}{}\n", "f{".repeat(nesting), "}".repeat(nesting));
        let first = luck_formatter::format(&source, luck_token::LuaVersion::Lua54, &options);
        if !first.errors.is_empty() {
            continue;
        }
        let second = luck_formatter::format(&first.output, luck_token::LuaVersion::Lua54, &options);
        assert!(second.errors.is_empty(), "nesting {nesting}: {:?}", second.errors.first());
        assert!(second.output == first.output, "nesting {nesting}: not idempotent");
    }
    assert!(formatted(&format!("return {}{}\n", "f{".repeat(127), "}".repeat(127))).len() > 254);
}
