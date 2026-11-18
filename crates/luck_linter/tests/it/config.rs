use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_linter::{
    Category, CyclomaticComplexityOptions, LintConfig, LintDiagnostic,
    RestrictedModulePathsOptions, Rule, RuleLevel, RuleSetting, Severity, analyze, lint_analyzed,
    lint_parsed, rule_named, rules,
};
use luck_token::LuaVersion;

use crate::common::{disable, enable, lint};

fn with_category(category: Category, level: RuleLevel) -> LintConfig {
    let mut config = LintConfig::default();
    config.categories.set_level(category, level);
    config
}

#[test]
fn rule_named_takes_every_registered_name_and_no_other() {
    for rule in rules() {
        assert_eq!(rule_named(rule.name()).map(Rule::name), Ok(rule.name()));
    }
    assert!(rule_named("not_a_real_rule").is_err());
}

#[test]
fn lint_distinguishes_luau_flavors() {
    let standalone_diags = lint("print(game)", LuaTarget::Luau, &LintConfig::default());
    assert!(
        standalone_diags
            .iter()
            .any(|diag| diag.rule == "undefined_variable" && diag.message.contains("game"))
    );
    let roblox_diags = lint("print(game)", LuaTarget::LuauRoblox, &LintConfig::default());
    assert!(roblox_diags.iter().all(|diag| diag.rule != "undefined_variable"));
}

#[test]
fn analyze_defines_the_extra_globals_of_its_config() {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, "return vim", LuaVersion::Lua54);
    let config = LintConfig::default();
    let flagged = lint_analyzed(&parse, &analyze(&parse, LuaTarget::Lua54, &config), &config);
    assert!(flagged.iter().any(|diag| diag.rule == "undefined_variable"), "{flagged:?}");

    let with_vim = LintConfig { extra_globals: vec!["vim".to_owned()], ..LintConfig::default() };
    let analysis = analyze(&parse, LuaTarget::Lua54, &with_vim);
    assert!(analysis.semantic().is_known_global("vim"));
    let quiet = lint_analyzed(&parse, &analysis, &with_vim);
    assert!(quiet.iter().all(|diag| diag.rule != "undefined_variable"), "{quiet:?}");
}

#[test]
fn lint_analyzed_reports_parse_errors_without_running_rules() {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, "local x =", LuaVersion::Lua54);
    let config = LintConfig::default();
    let diags = lint_analyzed(&parse, &analyze(&parse, LuaTarget::Lua54, &config), &config);
    assert!(!diags.is_empty(), "{diags:?}");
    assert!(diags.iter().all(|diag| diag.rule == "parse_error"), "{diags:?}");
}

#[test]
fn lint_parsed_reports_parse_errors_without_running_rules() {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, "local x =", LuaVersion::Lua54);
    let diags = lint_parsed(&parse, LuaTarget::Lua54, &LintConfig::default());
    assert!(!diags.is_empty(), "{diags:?}");
    assert!(diags.iter().all(|diag| diag.rule == "parse_error"), "{diags:?}");
}

#[test]
fn extra_globals_not_flagged() {
    let config = LintConfig { extra_globals: vec!["vim".to_owned()], ..Default::default() };
    let diags = lint("print(vim.fn)", LuaTarget::Lua54, &config);
    assert!(
        diags.iter().all(|diag| diag.rule != "undefined_variable" || !diag.message.contains("vim"))
    );
}

#[test]
fn rule_level_off_disables() {
    let config = disable("unused_variable");
    let diags = lint("local unused = 1", LuaTarget::Lua54, &config);
    assert!(diags.iter().all(|diag| diag.rule != "unused_variable"));
}

#[test]
fn categories_enable_group() {
    let config = with_category(Category::Suspicious, RuleLevel::Warn);
    let diags = lint("if true then end", LuaTarget::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "empty_block"));
}

#[test]
fn suspicious_rules_off_by_default() {
    let diags = lint("if true then end", LuaTarget::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.rule != "empty_block"));
}

#[test]
fn suspicious_rules_enabled_by_rule_level() {
    let config = enable("empty_block");
    let diags = lint("if true then end", LuaTarget::Lua54, &config);
    assert!(diags.iter().any(|diag| diag.rule == "empty_block"));
}

#[test]
fn correctness_off_silences_the_default_rules() {
    let config = with_category(Category::Correctness, RuleLevel::Off);
    let diags = lint("print(undefined_var)", LuaTarget::Lua54, &config);
    assert_eq!(diags, Vec::<LintDiagnostic>::new());
}

#[test]
fn rule_level_wins_over_its_category() {
    let mut config = with_category(Category::Correctness, RuleLevel::Off);
    config.rules.set_level("undefined_variable", RuleLevel::Warn);
    let diags = lint("print(undefined_var)", LuaTarget::Lua54, &config);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].rule, "undefined_variable");
    assert_eq!(diags[0].severity, Severity::Warning);
}

#[test]
fn category_level_sets_the_severity_of_its_rules() {
    let source = "\
local unused = 1
print(undefined_var)";
    let defaults = lint(source, LuaTarget::Lua54, &LintConfig::default());
    let default_severities: Vec<Severity> = defaults.iter().map(|diag| diag.severity).collect();
    assert_eq!(default_severities, [Severity::Warning, Severity::Error], "{defaults:?}");

    for (level, severity) in
        [(RuleLevel::Warn, Severity::Warning), (RuleLevel::Error, Severity::Error)]
    {
        let diags = lint(source, LuaTarget::Lua54, &with_category(Category::Correctness, level));
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
    let diags = lint(source, LuaTarget::Lua54, &config);
    assert!(
        diags.iter().any(|diag| diag.rule == "cyclomatic_complexity"),
        "expected cyclomatic_complexity to fire; got: {diags:?}"
    );
}

#[test]
fn cyclomatic_complexity_inert_without_threshold() {
    let config = enable("cyclomatic_complexity");
    let source = "function f() if a then end if b then end if c then end if d then end end";
    let diags = lint(source, LuaTarget::Lua54, &config);
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
    let diags = lint("require(\"forbidden.lib\")", LuaTarget::Lua54, &config);
    assert!(
        diags.iter().any(|diag| diag.rule == "restricted_module_paths"),
        "expected restricted_module_paths to fire; got: {diags:?}"
    );
}

#[test]
fn restricted_module_paths_inert_with_empty_list() {
    let diags = lint("require(\"anything\")", LuaTarget::Lua54, &LintConfig::default());
    assert!(
        diags.iter().all(|diag| diag.rule != "restricted_module_paths"),
        "expected no restricted_module_paths diagnostic with empty list; got: {diags:?}"
    );
}
