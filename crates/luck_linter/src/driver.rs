use luck_core::{LintConfig, LuaTarget};
use luck_semantic::SemanticAnalysis;
use luck_token::{LuaVersion, SourceError, StdlibEnvironment};
use rayon::prelude::*;

use crate::diagnostic::{Category, Finding, LintDiagnostic, Severity};
use crate::nodes::Nodes;
use crate::rule::{LintContext, NodeRule, Rule, WholeRule};
use crate::rules::{PARSE_ERROR_PSEUDO_RULE, RULES, RuleEntry};
use crate::{bus, suppression};

/// Lints `source` with the [`StdlibEnvironment::Standalone`] globals.
///
/// Roblox code needs [`lint_target`] with a Roblox `LuaTarget`.
#[must_use]
pub fn lint(source: &str, version: LuaVersion, config: &LintConfig) -> Vec<LintDiagnostic> {
    lint_parsed(
        &luck_parser::parse(source, version),
        version,
        StdlibEnvironment::Standalone,
        config,
    )
}

/// Lints `source` for `target`, whose stdlib environment decides which
/// globals exist.
#[must_use]
pub fn lint_target(source: &str, target: LuaTarget, config: &LintConfig) -> Vec<LintDiagnostic> {
    let version = target.lua_version();
    lint_parsed(&luck_parser::parse(source, version), version, target.stdlib_environment(), config)
}

/// Lints an existing parse, so hosts like the LSP don't parse twice.
///
/// A parse with errors yields one `parse_error` diagnostic per error, and no
/// rules run.
///
/// # Panics
///
/// In debug builds, when a rule's `node_types()` omits a type its hooks act on.
#[must_use]
pub fn lint_parsed(
    parse: &luck_parser::ParseResult,
    version: LuaVersion,
    environment: StdlibEnvironment,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    if !parse.errors.is_empty() {
        return parse.errors.iter().map(parse_error_diag).collect();
    }

    let semantic = analyze(&parse.block, version, environment, config);
    let nodes = Nodes::collect(&parse.block);
    let context = LintContext {
        block: &parse.block,
        blocks: nodes.blocks(),
        semantic: &semantic,
        source: &parse.source,
        comments: &parse.comments,
        config,
    };

    let mut diags = run_rules(&context, &nodes);
    suppression::apply(&mut diags, context.comments, context.source, &nodes);
    diags.sort_by_key(|diag| diag.span.start);
    diags
}

/// Runs semantic analysis with the config's extra globals seeded in.
pub(crate) fn analyze(
    block: &luck_ast::Block,
    version: LuaVersion,
    environment: StdlibEnvironment,
    config: &LintConfig,
) -> SemanticAnalysis {
    let mut semantic = luck_semantic::analyze_with_environment(block, version, environment);
    semantic.extra_globals.extend(config.extra_globals.iter().cloned());
    semantic
}

fn parse_error_diag(error: &SourceError) -> LintDiagnostic {
    LintDiagnostic {
        rule: PARSE_ERROR_PSEUDO_RULE,
        category: Category::Correctness,
        severity: Severity::Error,
        message: error.message.clone(),
        span: error.span,
        help: None,
        fix: None,
    }
}

/// Runs every enabled rule and returns the rules' findings as diagnostics, in
/// registry order.
///
/// Node rules share one bus pass while whole-tree rules run in parallel
/// beside it. Results go back into registry order, so the caller's stable
/// sort breaks ties the way a sequential run would. A node rule is skipped
/// when none of its node types occur in this file.
fn run_rules(context: &LintContext<'_>, nodes: &Nodes<'_>) -> Vec<LintDiagnostic> {
    let config = context.config;
    let mut whole_rules: Vec<(usize, &dyn WholeRule)> = Vec::new();
    let mut node_slots: Vec<usize> = Vec::new();
    let mut node_rules: Vec<&dyn NodeRule> = Vec::new();
    #[cfg(debug_assertions)]
    let mut skipped_node_rules: Vec<&dyn NodeRule> = Vec::new();
    for (slot, entry) in RULES.iter().enumerate() {
        if !is_rule_enabled(entry.rule(), config) {
            continue;
        }
        match *entry {
            RuleEntry::Whole(rule) => whole_rules.push((slot, rule)),
            RuleEntry::Node(rule) => {
                if !nodes.contains_any(rule.node_types()) {
                    #[cfg(debug_assertions)]
                    skipped_node_rules.push(rule);
                    continue;
                }
                node_slots.push(slot);
                node_rules.push(rule);
            }
        }
    }

    let (node_results, whole_results) = rayon::join(
        || bus::run(&node_rules, nodes.as_slice(), context),
        || {
            whole_rules
                .par_iter()
                .map(|&(slot, rule)| (slot, rule.check(context)))
                .collect::<Vec<_>>()
        },
    );

    #[cfg(debug_assertions)]
    bus::assert_dispatch_is_pure(
        &node_rules,
        &skipped_node_rules,
        &node_results,
        nodes.as_slice(),
        context,
    );

    let mut findings_by_slot = vec![Vec::new(); RULES.len()];
    for (slot, rule_findings) in
        whole_results.into_iter().chain(node_slots.into_iter().zip(node_results))
    {
        if let Some(slot_findings) = findings_by_slot.get_mut(slot) {
            *slot_findings = rule_findings;
        }
    }

    let mut diags: Vec<LintDiagnostic> = Vec::new();
    for (entry, rule_findings) in RULES.iter().zip(findings_by_slot) {
        if !rule_findings.is_empty() {
            diags.extend(into_diagnostics(entry.rule(), rule_findings, config));
        }
    }
    diags
}

/// `rule`'s findings as diagnostics, carrying its name and category and the
/// severity `config` resolves for it.
pub(crate) fn into_diagnostics(
    rule: &dyn Rule,
    findings: Vec<Finding>,
    config: &LintConfig,
) -> impl Iterator<Item = LintDiagnostic> {
    let name = rule.name();
    let category = rule.category();
    let severity = config
        .rule_overrides
        .get(name)
        .and_then(|setting| setting.severity)
        .unwrap_or_else(|| rule.default_severity());
    findings.into_iter().map(move |finding| finding.into_diagnostic(name, category, severity))
}

/// A rule's own `enabled` override wins. Otherwise `disable_default_rules`
/// turns every rule off, and without it correctness rules and the enabled
/// categories run.
fn is_rule_enabled(rule: &dyn Rule, config: &LintConfig) -> bool {
    if let Some(setting) = config.rule_overrides.get(rule.name())
        && let Some(enabled) = setting.enabled
    {
        return enabled;
    }
    if config.disable_default_rules {
        return false;
    }
    rule.category() == Category::Correctness || config.categories.contains(&rule.category())
}
