//! Rules firing, or staying quiet, through the full `lint` driver.

use luck_linter::{LintConfig, lint};
use luck_token::LuaVersion;

use crate::common::enable;

#[test]
fn detects_undefined_variable() {
    let diags = lint("print(undefined_var)", LuaVersion::Lua54, &LintConfig::default());
    assert!(
        diags
            .iter()
            .any(|diag| diag.rule == "undefined_variable" && diag.message.contains("undefined_var"))
    );
}

#[test]
fn named_vararg_not_flagged_undefined() {
    // Lua 5.5 `...name` declares a binding; its uses aren't globals.
    let diags = lint(
        "local function f(...args) return args end\nf(1)",
        LuaVersion::Lua55,
        &LintConfig::default(),
    );
    assert!(diags.iter().all(|diag| diag.rule != "undefined_variable"), "{diags:?}");
}

#[test]
fn known_globals_not_flagged() {
    let diags = lint("print(tostring(1))", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.rule != "undefined_variable"));
}

#[test]
fn detects_unused_variable() {
    let diags = lint("local unused = 1", LuaVersion::Lua54, &LintConfig::default());
    assert!(
        diags.iter().any(|diag| diag.rule == "unused_variable" && diag.message.contains("unused"))
    );
}

#[test]
fn underscore_prefix_skipped() {
    let diags = lint("local _unused = 1", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.rule != "unused_variable"));
}

#[test]
fn detects_setting_global() {
    let diags = lint("my_global = 1", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.rule == "setting_global"));
}

#[test]
fn detects_duplicate_keys() {
    let diags = lint("local t = { a = 1, a = 2 }", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.rule == "duplicate_keys"));
}

#[test]
fn detects_compare_nan() {
    let diags =
        lint("local x = 1\nif x == 0/0 then end", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.rule == "compare_nan"));
}

#[test]
fn detects_constant_table_comparison() {
    let diags =
        lint("local x = {}\nif x == {} then end", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.rule == "constant_table_comparison"));
}

#[test]
fn detects_almost_swapped() {
    let diags = lint("local a, b = 1, 2\na = b\nb = a", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.rule == "almost_swapped"));
}

#[test]
fn detects_type_check_inside_call() {
    let diags =
        lint("if type(x == \"string\") then end", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.rule == "type_check_inside_call"));
}

#[test]
fn detects_reversed_for_loop() {
    let config = enable("reversed_for_loop");
    let diags = lint("for i = 10, 1 do end", LuaVersion::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "reversed_for_loop"));
}

#[test]
fn detects_unbalanced_assignment() {
    let config = enable("unbalanced_assignment");
    let diags = lint("local a, b, c = 1, 2", LuaVersion::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "unbalanced_assignment"));
}

#[test]
fn detects_incorrect_stdlib_use_too_few() {
    let diags = lint("table.insert()", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.rule == "incorrect_stdlib_use"));
}

#[test]
fn detects_must_use() {
    let config = enable("must_use");
    let diags = lint("tostring(42)", LuaVersion::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "must_use"));
}
