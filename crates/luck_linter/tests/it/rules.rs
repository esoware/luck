use luck_linter::{FixKind, LintConfig, RuleId};
use luck_token::{LuaTarget, Span};

use crate::common::{enable, lint, lint_bytes};

/// A Latin-1 source lints as Lua reads it, and an escape of one of its bytes
/// names the byte.
#[test]
fn flags_a_lenient_escape_of_a_byte_that_is_not_utf8() {
    let source = b"print('caf\\\xE9') -- d\xE9j\xE0";
    let config = enable(RuleId::BadStringEscape);
    let diags = lint_bytes(source, LuaTarget::Lua51, &config, FixKind::empty());
    let found: Vec<(&str, &str, Span)> =
        diags.iter().map(|diag| (diag.code, &*diag.message, diag.span())).collect();
    assert_eq!(found, [("bad_string_escape", "invalid string escape `\\xe9`", Span::new(10, 12))]);
}

#[test]
fn flags_undefined_variable() {
    let diags = lint("print(undefined_var)", LuaTarget::Lua54, &LintConfig::default());
    assert!(
        diags
            .iter()
            .any(|diag| diag.code == "undefined_variable" && diag.message.contains("undefined_var"))
    );
}

#[test]
fn ignores_named_vararg_reads() {
    // Lua 5.5 `...name` declares a binding; its uses aren't globals.
    let diags = lint(
        "\
local function f(...args) return args end
f(1)",
        LuaTarget::Lua55,
        &LintConfig::default(),
    );
    assert!(diags.iter().all(|diag| diag.code != "undefined_variable"), "{diags:?}");
}

#[test]
fn ignores_known_globals() {
    let diags = lint("print(tostring(1))", LuaTarget::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.code != "undefined_variable"));
}

#[test]
fn flags_unused_variable() {
    let diags = lint("local unused = 1", LuaTarget::Lua54, &LintConfig::default());
    assert!(
        diags.iter().any(|diag| diag.code == "unused_variable" && diag.message.contains("unused"))
    );
}

#[test]
fn ignores_underscore_prefixed_unused_local() {
    let diags = lint("local _unused = 1", LuaTarget::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.code != "unused_variable"));
}

#[test]
fn flags_setting_global() {
    let diags = lint("my_global = 1", LuaTarget::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.code == "setting_global"));
}

#[test]
fn flags_duplicate_keys() {
    let diags = lint("local t = { a = 1, a = 2 }", LuaTarget::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.code == "duplicate_keys"));
}

#[test]
fn flags_compare_nan() {
    let diags = lint(
        "\
local x = 1
if x == 0/0 then end",
        LuaTarget::Lua54,
        &LintConfig::default(),
    );
    assert!(diags.iter().any(|diag| diag.code == "compare_nan"));
}

#[test]
fn flags_constant_table_comparison() {
    let diags = lint(
        "\
local x = {}
if x == {} then end",
        LuaTarget::Lua54,
        &LintConfig::default(),
    );
    assert!(diags.iter().any(|diag| diag.code == "constant_table_comparison"));
}

#[test]
fn flags_almost_swapped() {
    let diags = lint(
        "\
local a, b = 1, 2
a = b
b = a",
        LuaTarget::Lua54,
        &LintConfig::default(),
    );
    assert!(diags.iter().any(|diag| diag.code == "almost_swapped"));
}

#[test]
fn flags_type_check_inside_call() {
    let diags = lint("if type(x == \"string\") then end", LuaTarget::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.code == "type_check_inside_call"));
}

#[test]
fn flags_reversed_for_loop() {
    let config = enable(RuleId::ReversedForLoop);
    let diags = lint("for i = 10, 1 do end", LuaTarget::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.code == "reversed_for_loop"));
}

#[test]
fn flags_unbalanced_assignment() {
    let config = enable(RuleId::UnbalancedAssignment);
    let diags = lint("local a, b, c = 1, 2", LuaTarget::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.code == "unbalanced_assignment"));
}

#[test]
fn flags_incorrect_stdlib_use_with_too_few_args() {
    let diags = lint("table.insert()", LuaTarget::Lua54, &LintConfig::default());
    assert!(diags.iter().any(|diag| diag.code == "incorrect_stdlib_use"));
}

#[test]
fn flags_must_use() {
    let config = enable(RuleId::MustUse);
    let diags = lint("tostring(42)", LuaTarget::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.code == "must_use"));
}
