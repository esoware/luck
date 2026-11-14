use luck_core::LuaTarget;
use luck_linter::{
    Category, CyclomaticComplexityOptions, LintConfig, LintDiagnostic,
    RestrictedModulePathsOptions, RuleLevel, RuleSetting, Severity, lint, lint_analyzed,
    lint_target, unknown_rule_names,
};
use luck_token::LuaVersion;

use crate::common::{disable, enable};

fn with_category(category: Category, level: RuleLevel) -> LintConfig {
    let mut config = LintConfig::default();
    config.categories.set_level(category, level);
    config
}

#[test]
fn unknown_rule_names_flags_only_unregistered() {
    let mut config = LintConfig::default();
    config.rules.set_level("unused_variable", RuleLevel::Off);
    config.rules.set_level("cyclomatic_complexity", RuleLevel::Off);
    config.rules.set_level("not_a_real_rule", RuleLevel::Off);
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
fn lint_analyzed_reads_extra_globals_from_the_given_analysis() {
    let parse = luck_parser::parse("return vim", LuaVersion::Lua54);
    let mut semantic = luck_semantic::analyze(&parse.block, LuaVersion::Lua54);
    let config = LintConfig::default();
    let flagged = lint_analyzed(&parse, &semantic, &config);
    assert!(flagged.iter().any(|diag| diag.rule == "undefined_variable"), "{flagged:?}");

    semantic.extra_globals.insert("vim".to_owned());
    let quiet = lint_analyzed(&parse, &semantic, &config);
    assert!(quiet.iter().all(|diag| diag.rule != "undefined_variable"), "{quiet:?}");
}

#[test]
fn lint_analyzed_reports_parse_errors_without_running_rules() {
    let parse = luck_parser::parse("local x =", LuaVersion::Lua54);
    let semantic = luck_semantic::analyze(&parse.block, LuaVersion::Lua54);
    let diags = lint_analyzed(&parse, &semantic, &LintConfig::default());
    assert!(!diags.is_empty(), "{diags:?}");
    assert!(diags.iter().all(|diag| diag.rule == "parse_error"), "{diags:?}");
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
fn rule_level_off_disables() {
    let config = disable("unused_variable");
    let diags = lint("local unused = 1", LuaVersion::Lua54, &config);
    assert!(diags.iter().all(|diag| diag.rule != "unused_variable"));
}

#[test]
fn categories_enable_group() {
    let config = with_category(Category::Suspicious, RuleLevel::Warn);
    let diags = lint("if true then end", LuaVersion::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "empty_block"));
}

#[test]
fn suspicious_rules_off_by_default() {
    let diags = lint("if true then end", LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.rule != "empty_block"));
}

#[test]
fn suspicious_rules_enabled_by_rule_level() {
    let config = enable("empty_block");
    let diags = lint("if true then end", LuaVersion::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "empty_block"));
}

#[test]
fn correctness_off_silences_the_default_rules() {
    let config = with_category(Category::Correctness, RuleLevel::Off);
    let diags = lint("print(undefined_var)", LuaVersion::Lua54, &config);
    assert_eq!(diags, Vec::<LintDiagnostic>::new());
}

#[test]
fn rule_level_wins_over_its_category() {
    let mut config = with_category(Category::Correctness, RuleLevel::Off);
    config.rules.set_level("undefined_variable", RuleLevel::Warn);
    let diags = lint("print(undefined_var)", LuaVersion::Lua54, &config);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].rule, "undefined_variable");
    assert_eq!(diags[0].severity, Severity::Warning);
}

#[test]
fn category_level_sets_the_severity_of_its_rules() {
    let source = "\
local unused = 1
print(undefined_var)";
    let defaults = lint(source, LuaVersion::Lua54, &LintConfig::default());
    let default_severities: Vec<Severity> = defaults.iter().map(|diag| diag.severity).collect();
    assert_eq!(default_severities, [Severity::Warning, Severity::Error], "{defaults:?}");

    for (level, severity) in
        [(RuleLevel::Warn, Severity::Warning), (RuleLevel::Error, Severity::Error)]
    {
        let diags = lint(source, LuaVersion::Lua54, &with_category(Category::Correctness, level));
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert!(diags.iter().all(|diag| diag.severity == severity), "{diags:?}");
    }
}

#[test]
fn cyclomatic_complexity_fires_when_threshold_configured() {
    let mut config = LintConfig::default();
    config.rules.cyclomatic_complexity =
        Some(RuleSetting::WithOptions(RuleLevel::Warn, CyclomaticComplexityOptions { max: 2 }));
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
    let mut config = LintConfig::default();
    config.rules.restricted_module_paths = Some(RuleSetting::WithOptions(
        RuleLevel::Error,
        RestrictedModulePathsOptions { paths: vec!["forbidden.lib".to_owned()] },
    ));
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
