//! How `LintConfig` decides which rules run and what they see.

use luck_core::LuaTarget;
use luck_linter::{
    Category, LintConfig, LintDiagnostic, RuleSetting, lint, lint_target, unknown_rule_names,
};
use luck_token::LuaVersion;

use crate::common::{disable, enable};

#[test]
fn unknown_rule_names_flags_only_unregistered() {
    let mut config = LintConfig::default();
    config.rule_overrides.insert("unused_variable".to_owned(), RuleSetting::default());
    config.rule_overrides.insert("not_a_real_rule".to_owned(), RuleSetting::default());
    assert_eq!(unknown_rule_names(&config), vec!["not_a_real_rule"]);
}

#[test]
fn lint_target_distinguishes_luau_flavors() {
    let standalone_diags = lint_target("print(game)", LuaTarget::Luau, &LintConfig::default());
    assert!(
        standalone_diags
            .iter()
            .any(|diag| diag.rule == "undefined_variable" && diag.message.contains("game"))
    );
    let roblox_diags = lint_target("print(game)", LuaTarget::LuauRoblox, &LintConfig::default());
    assert!(roblox_diags.iter().all(|diag| diag.rule != "undefined_variable"));
}

#[test]
fn extra_globals_not_flagged() {
    let config = LintConfig { extra_globals: vec!["vim".to_owned()], ..Default::default() };
    let diags = lint("print(vim.fn)", LuaVersion::Lua54, &config);
    assert!(
        diags.iter().all(|diag| diag.rule != "undefined_variable" || !diag.message.contains("vim"))
    );
}

#[test]
fn rule_override_disables() {
    let config = disable("unused_variable");
    let diags = lint("local unused = 1", LuaVersion::Lua54, &config);
    assert!(diags.iter().all(|diag| diag.rule != "unused_variable"));
}

#[test]
fn categories_enable_group() {
    let config = LintConfig { categories: vec![Category::Suspicious], ..Default::default() };
    let diags = lint("if true then end", LuaVersion::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "empty_block"));
}

#[test]
fn suspicious_rules_off_by_default() {
    let diags = lint("if true then end", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.rule != "empty_block"));
}

#[test]
fn suspicious_rules_enabled_by_override() {
    let config = enable("empty_block");
    let diags = lint("if true then end", LuaVersion::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "empty_block"));
}

#[test]
fn disable_default_rules_silences_correctness() {
    let config = LintConfig { disable_default_rules: true, ..Default::default() };
    let diags = lint("print(undefined_var)", LuaVersion::Lua54, &config);
    assert_eq!(diags, Vec::<LintDiagnostic>::new());
}

#[test]
fn disable_default_rules_still_honors_explicit_enable() {
    let config = LintConfig { disable_default_rules: true, ..enable("undefined_variable") };
    let diags = lint("print(undefined_var)", LuaVersion::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "undefined_variable"));
}

#[test]
fn cyclomatic_complexity_fires_when_threshold_configured() {
    let config =
        LintConfig { max_cyclomatic_complexity: Some(2), ..enable("cyclomatic_complexity") };
    let source = "function f() if a then end if b then end if c then end if d then end end";
    let diags = lint(source, LuaVersion::Lua54, &config);
    assert!(
        diags.iter().any(|diag| diag.rule == "cyclomatic_complexity"),
        "expected cyclomatic_complexity to fire; got: {diags:?}"
    );
}

#[test]
fn cyclomatic_complexity_inert_without_threshold() {
    let config = enable("cyclomatic_complexity");
    let source = "function f() if a then end if b then end if c then end if d then end end";
    let diags = lint(source, LuaVersion::Lua54, &config);
    assert!(
        diags.iter().all(|diag| diag.rule != "cyclomatic_complexity"),
        "expected no cyclomatic_complexity diagnostic when threshold is None; got: {diags:?}"
    );
}

#[test]
fn restricted_module_paths_fires_when_configured() {
    let config = LintConfig {
        restricted_module_paths: vec!["forbidden.lib".to_owned()],
        ..Default::default()
    };
    let diags = lint("require(\"forbidden.lib\")", LuaVersion::Lua54, &config);
    assert!(
        diags.iter().any(|diag| diag.rule == "restricted_module_paths"),
        "expected restricted_module_paths to fire; got: {diags:?}"
    );
}

#[test]
fn restricted_module_paths_inert_with_empty_list() {
    let diags = lint("require(\"anything\")", LuaVersion::Lua54, &LintConfig::default());
    assert!(
        diags.iter().all(|diag| diag.rule != "restricted_module_paths"),
        "expected no restricted_module_paths diagnostic with empty list; got: {diags:?}"
    );
}
