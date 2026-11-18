use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_token::{LuaVersion, Span};

use crate::config::LintConfig;
use crate::diagnostic::{Category, Finding, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule};
use crate::rules::{RULES, RuleEntry};
use crate::{bus, driver, fix};

pub(crate) fn run_rule(rule: &dyn Rule, source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
    run_rule_with_config(rule, source, version, &LintConfig::default())
}

pub(crate) fn run_rule_roblox(rule: &dyn Rule, source: &str) -> Vec<LintDiagnostic> {
    check(rule, source, LuaTarget::LuauRoblox, &LintConfig::default())
}

pub(crate) fn run_rule_with_config(
    rule: &dyn Rule,
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    check(rule, source, standalone(version), config)
}

/// Applies the diagnostics' fixes the way `--fix` does, asserting that each
/// diagnostic carries one and that the result re-parses.
pub(crate) fn fix_and_reparse(
    source: &str,
    diags: &[LintDiagnostic],
    version: LuaVersion,
) -> String {
    assert!(diags.iter().all(|diag| diag.fix.is_some()), "{diags:?}");
    let fixed = fix::apply_one_pass(source, diags);
    let allocator = Allocator::new();
    let reparse = luck_parser::parse(&allocator, &fixed, version);
    assert!(reparse.errors.is_empty(), "reparse: {:?}", reparse.errors);
    fixed
}

const fn standalone(version: LuaVersion) -> LuaTarget {
    match version {
        LuaVersion::Lua51 => LuaTarget::Lua51,
        LuaVersion::Lua52 => LuaTarget::Lua52,
        LuaVersion::Lua53 => LuaTarget::Lua53,
        LuaVersion::Lua54 => LuaTarget::Lua54,
        LuaVersion::Lua55 => LuaTarget::Lua55,
        LuaVersion::Luau => LuaTarget::Luau,
    }
}

fn check(
    rule: &dyn Rule,
    source: &str,
    target: LuaTarget,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, source, target.lua_version());
    assert!(parse.errors.is_empty(), "parse: {:?}", parse.errors);
    let analysis = driver::analyze(&parse, target, config);
    let nodes = analysis.nodes();
    let context = LintContext::new(&parse, analysis.semantic(), nodes, config);
    let entry = RULES
        .iter()
        .find(|entry| entry.rule().name() == rule.name())
        .expect("only registered rules are tested");
    let findings = match *entry {
        RuleEntry::Whole(whole_rule) => whole_rule.check(&context),
        RuleEntry::Node(node_rule) => {
            bus::run(&[node_rule], nodes.as_slice(), &context).pop().unwrap_or_default()
        }
    };
    // A rule the config leaves off still runs here, at its own severity.
    let severity = driver::rule_severity(rule, config).unwrap_or_else(|| rule.default_severity());
    driver::into_diagnostics(entry.rule(), findings, severity).collect()
}

pub(crate) fn diagnostic(rule: &'static str, span: Span) -> LintDiagnostic {
    Finding::new("test", span).into_diagnostic(rule, Category::Correctness, Severity::Warning)
}
