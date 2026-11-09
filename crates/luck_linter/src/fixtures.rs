use std::sync::OnceLock;

use luck_core::LintConfig;
use luck_parser::ParseResult;
use luck_token::{LuaVersion, Span, StdlibEnvironment};

use crate::diagnostic::{Category, Finding, LintDiagnostic, Severity};
use crate::nodes::Nodes;
use crate::rule::{LintContext, Rule};
use crate::rules::{RULES, RuleEntry};
use crate::{bus, driver, fix};

pub(crate) fn run_rule(rule: &dyn Rule, source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
    run_rule_with_config(rule, source, version, &LintConfig::default())
}

pub(crate) fn run_rule_roblox(rule: &dyn Rule, source: &str) -> Vec<LintDiagnostic> {
    let version = LuaVersion::Luau;
    check(
        rule,
        &parse_cleanly(source, version),
        version,
        StdlibEnvironment::Roblox,
        &LintConfig::default(),
    )
}

pub(crate) fn run_rule_with_config(
    rule: &dyn Rule,
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    check(rule, &parse_cleanly(source, version), version, StdlibEnvironment::Standalone, config)
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
    let reparse = luck_parser::parse(&fixed, version);
    assert!(reparse.errors.is_empty(), "reparse: {:?}", reparse.errors);
    fixed
}

fn parse_cleanly(source: &str, version: LuaVersion) -> ParseResult {
    let parse = luck_parser::parse(source, version);
    assert!(parse.errors.is_empty(), "parse: {:?}", parse.errors);
    parse
}

fn check(
    rule: &dyn Rule,
    parse: &ParseResult,
    version: LuaVersion,
    environment: StdlibEnvironment,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    let semantic = driver::analyze(&parse.block, version, environment, config);
    let nodes = Nodes::collect(&parse.block);
    let context = LintContext {
        block: &parse.block,
        blocks: nodes.blocks(),
        function_blocks: nodes.function_blocks(),
        uninitialized_locals: nodes.uninitialized_locals(),
        semantic: &semantic,
        source: &parse.source,
        comments: &parse.comments,
        config,
        local_flow: OnceLock::new(),
    };
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
    driver::into_diagnostics(entry.rule(), findings, config).collect()
}

pub(crate) fn diagnostic(rule: &'static str, span: Span) -> LintDiagnostic {
    Finding::new("test", span).into_diagnostic(rule, Category::Correctness, Severity::Warning)
}
