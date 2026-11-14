use luck_minifier::TransformConfig;
use luck_token::LuaVersion;

use crate::common::{minify_lua54, minify_renaming_globals, minify_with, reparses};

#[test]
fn renames_locals() {
    let result = minify_lua54(
        "\
local longname = 1
return longname
",
    );
    assert!(!result.contains("longname"));
}

#[test]
fn preserves_globals() {
    let result = minify_lua54("print(42)\n");
    assert!(result.contains("print"));
}

#[test]
fn preserves_method_names() {
    let result = minify_lua54(
        "\
local t = {}
t:insert(1)
",
    );
    assert!(result.contains(":insert"));
}

#[test]
fn preserves_field_names() {
    let result = minify_lua54(
        "\
local t = { foo = 1 }
return t.foo
",
    );
    assert!(result.contains("foo"));
}

#[test]
fn shadowing_local_keeps_outer_global() {
    let result = minify_lua54(
        "\
print(1)
do
  local print = function() end
  print(2)
end
print(3)
",
    );
    assert!(result.contains("print"), "Global print must be preserved, got: {result}");
}

#[test]
fn disabled_rename_keeps_local_names() {
    let config = TransformConfig { rename_locals: false, ..Default::default() };
    let result = minify_with(
        "\
local longname = 1
print(longname)
return longname
",
        LuaVersion::Lua54,
        &config,
    );
    assert!(result.contains("longname"), "Rename should be disabled: {result}");
}

#[test]
fn renames_local_function_and_shrinks_output() {
    let source = "\
local function foo(a, b)
  local c = a + b
  return c * 2
end
return foo(1, 2)
";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.len() < source.len(), "Should be shorter: {result}");
    assert!(!result.contains("foo"), "Local function name 'foo' should be renamed: {result}");
}

#[test]
fn recursive_function_keeps_globals_and_fields() {
    let source = "\
local function fibonacci(n)
    if n <= 1 then return n end
    return fibonacci(n - 1) + fibonacci(n - 2)
end
local results = {}
for i = 1, 20 do
    table.insert(results, fibonacci(i))
end
return results
";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.contains("table"), "Global 'table' missing: {result}");
    assert!(result.contains("insert"), "Field 'insert' missing: {result}");
}

#[test]
fn captured_upvalue_renames_to_valid_lua() {
    let source = "\
local x = 1
local function inner()
  return x
end
return inner()
";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
}

#[test]
fn nested_scope_rename_keeps_globals() {
    let source = "\
local x = 1
do
  local y = x + 1
  print(y)
end
return x
";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.contains("print"), "Global 'print' must be preserved: {result}");
}

#[test]
fn rename_globals_keeps_name_read_before_assignment() {
    for (source, expected) in [
        (
            "\
t[g] = 1
g = 1
print(t, g)
",
            "t[g]=1 g=1 print(t,g)",
        ),
        (
            "\
g, t[g] = 1, 2
print(t, g)
",
            "g,t[g]=1,2 print(t,g)",
        ),
        (
            "\
if c then return g end
g = 1
print(g)
",
            "if c then return g end g=1 print(g)",
        ),
        (
            "\
function g.x() end
g = 1
print(g)
",
            "function g.x()end g=1 print(g)",
        ),
    ] {
        let result = minify_renaming_globals(source, LuaVersion::Lua54);
        assert_eq!(result, expected, "source: {source}");
        assert_eq!(
            minify_renaming_globals(&result, LuaVersion::Lua54),
            result,
            "must be idempotent"
        );
    }
}

#[test]
fn rename_globals_keeps_env() {
    for (source, expected) in [
        (
            "\
_ENV = {print = print}
print(1)
",
            "_ENV={print=print}print(1)",
        ),
        (
            "\
local _ENV = {print = print}
print(1)
",
            "local _ENV={print=print}print(1)",
        ),
        (
            "for _, _ENV in ipairs({{print = print}}) do print(1) end\n",
            "for l,_ENV in ipairs{{print=print}}do print(1)end",
        ),
    ] {
        let result = minify_renaming_globals(source, LuaVersion::Lua54);
        assert_eq!(result, expected, "source: {source}");
        let second = minify_renaming_globals(&result, LuaVersion::Lua54);
        assert_eq!(second, result, "must be idempotent");
    }
}

#[test]
fn rename_globals_keeps_names_a_global_declaration_declares() {
    for (source, expected) in [
        (
            "\
global g
g = 1
",
            "global g g=1",
        ),
        (
            "\
g = 1
global g
",
            "g=1 global g",
        ),
        (
            "\
g = 1
global function g() end
",
            "g=1 global function g()end",
        ),
        (
            "\
function g() end
global g
g()
",
            "function g()end global g g()",
        ),
    ] {
        let result = minify_renaming_globals(source, LuaVersion::Lua55);
        assert_eq!(result, expected, "source: {source}");
        let second = minify_renaming_globals(&result, LuaVersion::Lua55);
        assert_eq!(second, result, "must be idempotent");
    }
}

#[test]
fn rename_globals_keeps_name_read_before_function_definition() {
    for (source, expected) in [
        (
            "\
print(g)
function g() end
",
            "print(g)function g()end",
        ),
        (
            "\
function g() end
print(g)
",
            "function l()end print(l)",
        ),
        // The first round gives the local and the function the same name,
        // and the later rounds must still tell them apart.
        (
            "\
do local x = f()
print(x)
end
function y() end
print(y)
",
            "do local l=f()print(l)end function l()end print(l)",
        ),
    ] {
        let result = minify_renaming_globals(source, LuaVersion::Lua54);
        assert_eq!(result, expected, "source: {source}");
        let second = minify_renaming_globals(&result, LuaVersion::Lua54);
        assert_eq!(second, result, "must be idempotent");
    }
}
