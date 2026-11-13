use luck_minifier::TransformConfig;
use luck_token::LuaVersion;

use crate::common::{minify_lua54, minify_renaming_globals, minify_with, reparses};

#[test]
fn renames_locals() {
    let result = minify_lua54("local longname = 1\nreturn longname\n");
    assert!(!result.contains("longname"));
}

#[test]
fn preserves_globals() {
    let result = minify_lua54("print(42)\n");
    assert!(result.contains("print"));
}

#[test]
fn preserves_method_names() {
    let result = minify_lua54("local t = {}\nt:insert(1)\n");
    assert!(result.contains(":insert"));
}

#[test]
fn preserves_field_names() {
    let result = minify_lua54("local t = { foo = 1 }\nreturn t.foo\n");
    assert!(result.contains("foo"));
}

#[test]
fn shadowing_local_keeps_outer_global() {
    let result =
        minify_lua54("print(1)\ndo\n  local print = function() end\n  print(2)\nend\nprint(3)\n");
    assert!(result.contains("print"), "Global print must be preserved, got: {result}");
}

#[test]
fn disabled_rename_keeps_local_names() {
    let config = TransformConfig { rename_locals: false, ..Default::default() };
    let result = minify_with(
        "local longname = 1\nprint(longname)\nreturn longname\n",
        LuaVersion::Lua54,
        &config,
    );
    assert!(result.contains("longname"), "Rename should be disabled: {result}");
}

#[test]
fn renames_local_function_and_shrinks_output() {
    let source =
        "local function foo(a, b)\n  local c = a + b\n  return c * 2\nend\nreturn foo(1, 2)\n";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.len() < source.len(), "Should be shorter: {result}");
    assert!(!result.contains("foo"), "Local function name 'foo' should be renamed: {result}");
}

#[test]
fn recursive_function_keeps_globals_and_fields() {
    let source = "local function fibonacci(n)\n    if n <= 1 then return n end\n    return fibonacci(n - 1) + fibonacci(n - 2)\nend\nlocal results = {}\nfor i = 1, 20 do\n    table.insert(results, fibonacci(i))\nend\nreturn results\n";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.contains("table"), "Global 'table' missing: {result}");
    assert!(result.contains("insert"), "Field 'insert' missing: {result}");
}

#[test]
fn captured_upvalue_renames_to_valid_lua() {
    let source = "local x = 1\nlocal function inner()\n  return x\nend\nreturn inner()\n";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
}

#[test]
fn nested_scope_rename_keeps_globals() {
    let source = "local x = 1\ndo\n  local y = x + 1\n  print(y)\nend\nreturn x\n";
    let result = minify_lua54(source);
    assert!(reparses(&result), "Parse errors\nOutput: {result}");
    assert!(result.contains("print"), "Global 'print' must be preserved: {result}");
}

#[test]
fn rename_globals_keeps_name_read_before_assignment() {
    for (source, expected) in [
        ("t[g] = 1\ng = 1\nprint(t, g)\n", "t[g]=1 g=1 print(t,g)"),
        ("g, t[g] = 1, 2\nprint(t, g)\n", "g,t[g]=1,2 print(t,g)"),
        ("if c then return g end\ng = 1\nprint(g)\n", "if c then return g end g=1 print(g)"),
        ("function g.x() end\ng = 1\nprint(g)\n", "function g.x()end g=1 print(g)"),
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
        ("_ENV = {print = print}\nprint(1)\n", "_ENV={print=print}print(1)"),
        ("local _ENV = {print = print}\nprint(1)\n", "local _ENV={print=print}print(1)"),
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
        ("global g\ng = 1\n", "global g g=1"),
        ("g = 1\nglobal g\n", "g=1 global g"),
        ("g = 1\nglobal function g() end\n", "g=1 global function g()end"),
        ("function g() end\nglobal g\ng()\n", "function g()end global g g()"),
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
        ("print(g)\nfunction g() end\n", "print(g)function g()end"),
        ("function g() end\nprint(g)\n", "function l()end print(l)"),
        // The first round gives the local and the function the same name,
        // and the later rounds must still tell them apart.
        (
            "do local x = f()\nprint(x)\nend\nfunction y() end\nprint(y)\n",
            "do local l=f()print(l)end function l()end print(l)",
        ),
    ] {
        let result = minify_renaming_globals(source, LuaVersion::Lua54);
        assert_eq!(result, expected, "source: {source}");
        let second = minify_renaming_globals(&result, LuaVersion::Lua54);
        assert_eq!(second, result, "must be idempotent");
    }
}
