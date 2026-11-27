use luck_allocator::Allocator;
use luck_diagnostics::Severity;
use luck_linter::{Category, FixKind, LintConfig, Linter, Message, RuleId, RuleLevel};
use luck_stdlib::{SecurityLevel, luau_library, roblox_library};

use crate::common::{disable, enable, lint, lint_bytes};

fn with_category(category: Category, level: RuleLevel) -> LintConfig {
    let mut config = LintConfig::default();
    config.categories.set_level(category, level);
    config
}

#[test]
fn a_rule_parses_from_its_name_and_no_other_name_parses() {
    for rule in RuleId::ALL {
        assert_eq!(rule.name().parse(), Ok(rule));
        assert_eq!(RuleId::from_name(rule.name()), Some(rule));
    }
    assert_eq!(
        "not_a_real_rule".parse::<RuleId>(),
        Err("unknown lint rule `not_a_real_rule`".to_owned())
    );
}

#[test]
fn game_is_a_global_of_roblox_only() {
    let config = LintConfig::default();
    let standalone_diags = lint("print(game)", &config);
    assert!(
        standalone_diags
            .iter()
            .any(|diag| diag.code == "undefined_variable" && diag.message.contains("game"))
    );
    let roblox_diags =
        lint_bytes(b"print(game)", roblox_library(SecurityLevel::None), &config, FixKind::empty());
    assert!(roblox_diags.iter().all(|diag| diag.code != "undefined_variable"));
}

#[test]
fn analyze_defines_the_extra_globals_of_its_config() {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, b"return app");
    let linter = Linter::new(LintConfig::default());
    let without_app = linter.analyze(&parse, luau_library());
    let flagged = linter
        .run_analyzed(&without_app, FixKind::empty(), None, &|| false)
        .expect("nothing cancels it");
    assert!(flagged.iter().any(|diag| diag.code == "undefined_variable"), "{flagged:?}");

    let with_app =
        Linter::new(LintConfig { extra_globals: vec!["app".to_owned()], ..LintConfig::default() });
    let analysis = with_app.analyze(&parse, luau_library());
    assert!(analysis.semantic().is_known_global("app"));
    let quiet = with_app
        .run_analyzed(&analysis, FixKind::empty(), None, &|| false)
        .expect("nothing cancels it");
    assert!(quiet.iter().all(|diag| diag.code != "undefined_variable"), "{quiet:?}");
}

#[test]
fn run_analyzed_reports_parse_errors_without_running_rules() {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, b"local x =");
    let linter = Linter::new(LintConfig::default());
    let analysis = linter.analyze(&parse, luau_library());
    let diags = linter
        .run_analyzed(&analysis, FixKind::empty(), None, &|| true)
        .expect("a parse error is reported before any rule could be cancelled");
    assert!(!diags.is_empty(), "{diags:?}");
    assert!(diags.iter().all(|diag| diag.code == "E008"), "{diags:?}");
}

#[test]
fn run_reports_parse_errors_without_running_rules() {
    let diags = lint("local x =", &LintConfig::default());
    assert!(!diags.is_empty(), "{diags:?}");
    assert!(diags.iter().all(|diag| diag.code == "E008"), "{diags:?}");
}

#[test]
fn run_defines_the_extra_globals_of_its_config() {
    let config = LintConfig { extra_globals: vec!["app".to_owned()], ..Default::default() };
    let diags = lint("print(app.fn)", &config);
    assert!(
        diags.iter().all(|diag| diag.code != "undefined_variable" || !diag.message.contains("app"))
    );
}

#[test]
fn an_off_rule_level_turns_a_default_rule_off() {
    let config = disable(RuleId::UnusedVariable);
    let diags = lint("local unused = 1", &config);
    assert!(diags.iter().all(|diag| diag.code != "unused_variable"));
}

#[test]
fn a_category_level_runs_its_rules() {
    let config = with_category(Category::Suspicious, RuleLevel::Warn);
    let diags = lint("if true then end", &config);
    assert!(diags.iter().any(|diag| diag.code == "empty_block"));
}

#[test]
fn suspicious_rules_off_by_default() {
    let diags = lint("if true then end", &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.code != "empty_block"));
}

#[test]
fn suspicious_rules_enabled_by_rule_level() {
    let config = enable(RuleId::EmptyBlock);
    let diags = lint("if true then end", &config);
    assert!(diags.iter().any(|diag| diag.code == "empty_block"));
}

#[test]
fn correctness_off_silences_the_default_rules() {
    let config = with_category(Category::Correctness, RuleLevel::Off);
    let diags = lint("print(undefined_var)", &config);
    assert_eq!(diags, Vec::<Message>::new());
}

#[test]
fn rule_level_wins_over_its_category() {
    let mut config = with_category(Category::Correctness, RuleLevel::Off);
    config.rules.set_level(RuleId::UndefinedVariable, RuleLevel::Warn);
    let diags = lint("print(undefined_var)", &config);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "undefined_variable");
    assert_eq!(diags[0].severity, Severity::Warning);
}

#[test]
fn category_level_sets_the_severity_of_its_rules() {
    let source = "\
local unused = 1
print(undefined_var)";
    let defaults = lint(source, &LintConfig::default());
    let default_severities: Vec<Severity> = defaults.iter().map(|diag| diag.severity).collect();
    assert_eq!(default_severities, [Severity::Warning, Severity::Error], "{defaults:?}");

    for (level, severity) in
        [(RuleLevel::Warn, Severity::Warning), (RuleLevel::Error, Severity::Error)]
    {
        let diags = lint(source, &with_category(Category::Correctness, level));
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert!(diags.iter().all(|diag| diag.severity == severity), "{diags:?}");
    }
}

/// The `lint` section `lint_json` gives.
fn parsed(lint_json: &str) -> LintConfig {
    serde_json::from_str(lint_json).expect("a valid `lint` section")
}

#[test]
fn cyclomatic_complexity_fires_when_threshold_configured() {
    let config = parsed(r#"{ "rules": { "cyclomatic_complexity": ["warn", { "max": 2 }] } }"#);
    let source = "function f() if a then end if b then end if c then end if d then end end";
    let diags = lint(source, &config);
    assert!(
        diags.iter().any(|diag| diag.code == "cyclomatic_complexity"),
        "expected cyclomatic_complexity to fire; got: {diags:?}"
    );
}

#[test]
fn cyclomatic_complexity_inert_without_threshold() {
    let config = enable(RuleId::CyclomaticComplexity);
    let source = "function f() if a then end if b then end if c then end if d then end end";
    let diags = lint(source, &config);
    assert!(
        diags.iter().all(|diag| diag.code != "cyclomatic_complexity"),
        "expected no cyclomatic_complexity diagnostic when threshold is None; got: {diags:?}"
    );
}

#[test]
fn restricted_module_paths_fires_when_configured() {
    let config = parsed(
        r#"{ "rules": { "restricted_module_paths": ["error", { "paths": ["./forbidden/lib"] }] } }"#,
    );
    let diags = lint("require(\"./forbidden/lib\")", &config);
    assert!(
        diags.iter().any(|diag| diag.code == "restricted_module_paths"),
        "expected restricted_module_paths to fire; got: {diags:?}"
    );
}

#[test]
fn restricted_module_paths_inert_with_empty_list() {
    let diags = lint("require(\"./anything\")", &LintConfig::default());
    assert!(
        diags.iter().all(|diag| diag.code != "restricted_module_paths"),
        "expected no restricted_module_paths diagnostic with empty list; got: {diags:?}"
    );
}
