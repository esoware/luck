use luck_core::TransformConfig;
use luck_core::types::LuaTarget;
use luck_minifier::minify;

fn minify_lua54(source: &str) -> String {
    minify(
        source,
        LuaTarget::Lua54,
        &TransformConfig::default(),
        "<test>",
    )
    .expect("minify failed")
}

fn reparses(source: &str) -> bool {
    let result = luck_parser::parse(source, LuaTarget::Lua54.lua_version());
    result.errors.is_empty()
}

const MODULE_SOURCE: &str = r#"
local M = {}
function M.greet(name)
    local greeting = "Hello, " .. name .. "!"
    print(greeting)
    return greeting
end
function M.add(a, b)
    local result = a + b
    return result
end
local unused = 42
return M
"#;

#[test]
fn module_preserves_semantics() {
    let result = minify_lua54(MODULE_SOURCE);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(
        result.len() < MODULE_SOURCE.len(),
        "Not shorter\nOutput: {result}"
    );
    assert!(result.contains("greet"), "Field 'greet' missing: {result}");
    assert!(result.contains("add"), "Field 'add' missing: {result}");
    assert!(result.contains("print"), "Global 'print' missing: {result}");
    assert!(
        !result.contains("unused"),
        "Dead local 'unused' not removed: {result}"
    );
    let return_count = result.matches("return").count();
    assert!(
        return_count >= 3,
        "Expected >=3 returns, got {return_count}: {result}"
    );
    let func_count = result.matches("function").count();
    assert!(
        func_count >= 2,
        "Expected >=2 function keywords, got {func_count}: {result}"
    );
}

const CONTROL_FLOW_SOURCE: &str = r#"
local function process(items)
    if items == nil then
        return {}
    end
    local results = {}
    for i = 1, #items do
        local item = items[i]
        if item > 0 then
            if item < 100 then
                results[#results + 1] = item * 2
            else
                results[#results + 1] = 100
            end
        end
    end
    return results
end
return process
"#;

#[test]
fn control_flow_preserves_semantics() {
    let result = minify_lua54(CONTROL_FLOW_SOURCE);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(
        result.len() < CONTROL_FLOW_SOURCE.len(),
        "Not shorter\nOutput: {result}"
    );
    assert!(result.contains("nil"), "nil check missing: {result}");
    assert!(result.contains("for"), "for loop missing: {result}");
    let return_count = result.matches("return").count();
    assert!(
        return_count >= 3,
        "Expected >=3 returns, got {return_count}: {result}"
    );
}

const STRING_OPS_SOURCE: &str = r#"
local config = {
    debug = false,
    verbose = true,
    timeout = 30,
    name = "test",
}
local function format_config(cfg)
    local parts = {}
    for key, value in pairs(cfg) do
        parts[#parts + 1] = key .. "=" .. tostring(value)
    end
    return table.concat(parts, ", ")
end
return format_config(config)
"#;

#[test]
fn string_ops_preserves_semantics() {
    let result = minify_lua54(STRING_OPS_SOURCE);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(
        result.len() < STRING_OPS_SOURCE.len(),
        "Not shorter\nOutput: {result}"
    );
    assert!(result.contains("pairs"), "Global 'pairs' missing: {result}");
    assert!(
        result.contains("tostring"),
        "Global 'tostring' missing: {result}"
    );
    assert!(result.contains("table"), "Global 'table' missing: {result}");
    assert!(
        result.contains("concat"),
        "Method 'concat' missing: {result}"
    );
    assert!(result.contains("debug"), "Field 'debug' missing: {result}");
    assert!(
        result.contains("verbose"),
        "Field 'verbose' missing: {result}"
    );
    assert!(
        result.contains("timeout"),
        "Field 'timeout' missing: {result}"
    );
    let func_count = result.matches("function").count();
    assert!(
        func_count >= 1,
        "Expected >=1 function keyword, got {func_count}: {result}"
    );
}

fn verify_idempotent(source: &str, label: &str) {
    let first = minify_lua54(source);
    let second = minify_lua54(&first);
    assert!(
        reparses(&second),
        "Re-minified {label} should parse: {second}"
    );
    assert!(
        second.len() <= first.len(),
        "Re-minification of {label} should not grow: first={}, second={}",
        first.len(),
        second.len()
    );
}

#[test]
fn integration_sources_are_idempotent() {
    verify_idempotent(MODULE_SOURCE, "module");
    verify_idempotent(CONTROL_FLOW_SOURCE, "control_flow");
    verify_idempotent(STRING_OPS_SOURCE, "string_ops");
}

const DEAD_CODE_SOURCE: &str = r#"
local function used() return 1 end
local function unused_func() return 2 end
local dead_var = "never read"
local x = used()
if true then
    x = x + 1
end
if false then
    x = x + 100
end
return x
"#;

#[test]
fn dead_code_eliminates_correctly() {
    let result = minify_lua54(DEAD_CODE_SOURCE);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(
        result.len() < DEAD_CODE_SOURCE.len(),
        "Not shorter\nOutput: {result}"
    );
    assert!(
        !result.contains("unused_func"),
        "unused_func not eliminated: {result}"
    );
    assert!(
        !result.contains("never read"),
        "Dead string not eliminated: {result}"
    );
    assert!(
        !result.contains("100"),
        "if-false branch not eliminated: {result}"
    );
    let return_count = result.matches("return").count();
    assert!(return_count >= 1, "Expected >=1 return: {result}");
}

#[test]
fn dead_code_is_idempotent() {
    let first = minify_lua54(DEAD_CODE_SOURCE);
    let second = minify_lua54(&first);
    assert_eq!(first, second);
}
