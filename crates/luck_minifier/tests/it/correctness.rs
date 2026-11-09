use std::thread::Builder;

use crate::common::{minify_lua54, reparses};

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
    assert!(result.len() < MODULE_SOURCE.len(), "Not shorter\nOutput: {result}");
    assert!(result.contains("greet"), "Field 'greet' missing: {result}");
    assert!(result.contains("add"), "Field 'add' missing: {result}");
    assert!(result.contains("print"), "Global 'print' missing: {result}");
    assert!(!result.contains("unused"), "Dead local 'unused' not removed: {result}");
    let return_count = result.matches("return").count();
    assert!(return_count >= 3, "Expected >=3 returns, got {return_count}: {result}");
    let function_count = result.matches("function").count();
    assert!(function_count >= 2, "Expected >=2 function keywords, got {function_count}: {result}");
}

const CONTROL_FLOW_SOURCE: &str = "
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
";

#[test]
fn control_flow_preserves_semantics() {
    let result = minify_lua54(CONTROL_FLOW_SOURCE);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.len() < CONTROL_FLOW_SOURCE.len(), "Not shorter\nOutput: {result}");
    assert!(result.contains("nil"), "nil check missing: {result}");
    assert!(result.contains("for"), "for loop missing: {result}");
    let return_count = result.matches("return").count();
    assert!(return_count >= 3, "Expected >=3 returns, got {return_count}: {result}");
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
    assert!(result.len() < STRING_OPS_SOURCE.len(), "Not shorter\nOutput: {result}");
    assert!(result.contains("pairs"), "Global 'pairs' missing: {result}");
    assert!(result.contains("tostring"), "Global 'tostring' missing: {result}");
    assert!(result.contains("table"), "Global 'table' missing: {result}");
    assert!(result.contains("concat"), "Field 'concat' missing: {result}");
    assert!(result.contains("debug"), "Field 'debug' missing: {result}");
    assert!(result.contains("verbose"), "Field 'verbose' missing: {result}");
    assert!(result.contains("timeout"), "Field 'timeout' missing: {result}");
    let function_count = result.matches("function").count();
    assert!(function_count >= 1, "Expected >=1 function keyword, got {function_count}: {result}");
}

fn assert_reminifying_does_not_grow(source: &str, label: &str) {
    let first = minify_lua54(source);
    let second = minify_lua54(&first);
    assert!(reparses(&second), "Re-minified {label} should parse: {second}");
    assert!(
        second.len() <= first.len(),
        "Re-minification of {label} should not grow: first={}, second={}",
        first.len(),
        second.len()
    );
}

#[test]
fn reminifying_sample_sources_does_not_grow_them() {
    assert_reminifying_does_not_grow(MODULE_SOURCE, "module");
    assert_reminifying_does_not_grow(CONTROL_FLOW_SOURCE, "control_flow");
    assert_reminifying_does_not_grow(STRING_OPS_SOURCE, "string_ops");
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
    assert!(result.len() < DEAD_CODE_SOURCE.len(), "Not shorter\nOutput: {result}");
    assert!(!result.contains("unused_func"), "unused_func not eliminated: {result}");
    assert!(!result.contains("never read"), "Dead string not eliminated: {result}");
    assert!(!result.contains("100"), "if-false branch not eliminated: {result}");
    let return_count = result.matches("return").count();
    assert!(return_count >= 1, "Expected >=1 return: {result}");
}

#[test]
fn dead_code_is_idempotent() {
    let first = minify_lua54(DEAD_CODE_SOURCE);
    let second = minify_lua54(&first);
    assert_eq!(first, second);
}

#[test]
fn reminifying_does_not_grow_output() {
    let source = "local function add(a, b)\n  return a + b\nend\nlocal x = add(1, 2)\nprint(x)\n";
    let first = minify_lua54(source);
    let second = minify_lua54(&first);
    // Re-minification may rename locals differently (naming is
    // frequency-based), but the output length must not grow.
    assert!(
        second.len() <= first.len(),
        "Re-minification should not grow output: first={}, second={}",
        first.len(),
        second.len()
    );
    assert!(reparses(&second), "Re-minified output should re-parse: {second}");
}

#[test]
fn table_code_keeps_globals_and_fields() {
    let source = "local function make_table(n)\n    local t = {}\n    for i = 1, n do\n        t[i] = { value = i * 2, label = \"item\" .. tostring(i) }\n    end\n    return t\nend\nlocal items = make_table(10)\nif #items > 5 then\n    print(\"many items\")\nelse\n    print(\"few items\")\nend\nfor _, v in ipairs(items) do\n    if v.value > 10 then\n        print(v.label)\n    end\nend\nreturn items\n";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.contains("tostring"), "Global 'tostring' missing: {result}");
    assert!(result.contains("ipairs"), "Global 'ipairs' missing: {result}");
    assert!(result.contains("print"), "Global 'print' missing: {result}");
    assert!(result.contains("value"), "Field 'value' missing: {result}");
    assert!(result.contains("label"), "Field 'label' missing: {result}");
}

fn minify_on_a_cli_sized_stack(source: String) -> String {
    Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || minify_lua54(&source))
        .expect("spawn minify thread")
        .join()
        .expect("minify thread panicked")
}

#[test]
fn bug_long_if_return_chain_does_not_overflow_the_stack() {
    let source = format!("{}return 0\n", "if a then return 1 end\n".repeat(6000));
    let output = minify_on_a_cli_sized_stack(source);
    assert!(reparses(&output), "flat guards must minify to valid Lua, not abort the process");
}

#[test]
fn bug_long_operator_chain_does_not_overflow_the_stack() {
    let source = format!("return a{}\n", " + a".repeat(6000));
    let output = minify_on_a_cli_sized_stack(source);
    assert!(reparses(&output), "a chain the parser accepts must not abort the process");
}
