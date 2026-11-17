use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_parser::ParseResult;
use luck_semantic::SemanticAnalysis;
use luck_token::SourceError;
use rayon::prelude::*;

use crate::config::{LintConfig, RuleLevel};
use crate::diagnostic::{Category, Finding, LintDiagnostic, Severity};
use crate::nodes::Nodes;
use crate::rule::{LintContext, NodeRule, Rule, WholeRule};
use crate::rules::{PARSE_ERROR_PSEUDO_RULE, RULES, RuleEntry};
use crate::{bus, suppression};

/// Parses `source` into `allocator` and lints it for `target`, whose stdlib
/// environment decides which globals exist.
#[must_use]
pub fn lint(
    allocator: &Allocator,
    source: &str,
    target: LuaTarget,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    lint_parsed(&luck_parser::parse(allocator, source, target.lua_version()), target, config)
}

/// Analyzes `parse` for `target` and lints it. A parse with errors yields
/// one `parse_error` diagnostic per error, and no rules run.
///
/// # Panics
///
/// In debug builds, when a rule's `node_types()` omits a type its hooks act on.
#[must_use]
pub fn lint_parsed(
    parse: &ParseResult<'_>,
    target: LuaTarget,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    if !parse.errors.is_empty() {
        return parse_error_diags(parse);
    }
    lint_analyzed(parse, &analyze(parse, target, config), config)
}

/// Lints an existing parse and analysis, so hosts like the LSP don't build
/// either twice.
///
/// `semantic` must be `parse`'s and already hold `config.extra_globals`,
/// because rules read them from it. A parse with errors yields one
/// `parse_error` diagnostic per error, and no rules run.
///
/// # Panics
///
/// In debug builds, when a rule's `node_types()` omits a type its hooks act on.
#[must_use]
pub fn lint_analyzed(
    parse: &ParseResult<'_>,
    semantic: &SemanticAnalysis<'_>,
    config: &LintConfig,
) -> Vec<LintDiagnostic> {
    if !parse.errors.is_empty() {
        return parse_error_diags(parse);
    }

    let nodes = Nodes::collect(&parse.block);
    let context = LintContext::new(parse, semantic, &nodes, config);

    let mut diags = run_rules(&context, &nodes);
    suppression::apply(&mut diags, context.comments, context.source, &nodes, semantic.version());
    diags.sort_by_key(|diag| diag.span.start);
    diags
}

pub(crate) fn analyze<'ast>(
    parse: &'ast ParseResult<'_>,
    target: LuaTarget,
    config: &LintConfig,
) -> SemanticAnalysis<'ast> {
    let mut semantic =
        luck_semantic::analyze(&parse.block, target.lua_version(), target.stdlib_environment());
    semantic.add_extra_globals(config.extra_globals.iter().map(String::as_str));
    semantic
}

fn parse_error_diags(parse: &ParseResult<'_>) -> Vec<LintDiagnostic> {
    parse.errors.iter().map(parse_error_diag).collect()
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

/// Node rules share one bus pass while whole-tree rules run beside it, on
/// the rayon pool for a file large enough to gain from it.
///
/// Results go back into registry order, so the caller's stable sort breaks
/// ties the way a sequential run would. A node rule is skipped when none of
/// its node types occur in this file.
fn run_rules(context: &LintContext<'_>, nodes: &Nodes<'_>) -> Vec<LintDiagnostic> {
    let config = context.config;
    let mut severities: Vec<Option<Severity>> = Vec::with_capacity(RULES.len());
    let mut whole_rules: Vec<(usize, &dyn WholeRule)> = Vec::new();
    let mut node_slots: Vec<usize> = Vec::new();
    let mut node_rules: Vec<&dyn NodeRule> = Vec::new();
    #[cfg(debug_assertions)]
    let mut skipped_node_rules: Vec<&dyn NodeRule> = Vec::new();
    for (slot, entry) in RULES.iter().enumerate() {
        let severity = rule_severity(entry.rule(), config);
        severities.push(severity);
        if severity.is_none() {
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

    let check_whole = |&(slot, rule): &(usize, &dyn WholeRule)| (slot, rule.check(context));
    let ast_nodes = nodes.as_slice();
    let (node_results, whole_results): (_, Vec<_>) = if ast_nodes.len() < bus::MIN_PARALLEL_NODES {
        (bus::run(&node_rules, ast_nodes, context), whole_rules.iter().map(check_whole).collect())
    } else {
        rayon::join(
            || bus::run_in_parallel(&node_rules, ast_nodes, context),
            || whole_rules.par_iter().map(check_whole).collect(),
        )
    };

    #[cfg(debug_assertions)]
    bus::assert_dispatch_is_pure(
        &node_rules,
        &skipped_node_rules,
        &node_results,
        ast_nodes,
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
    for ((entry, rule_findings), slot_severity) in
        RULES.iter().zip(findings_by_slot).zip(severities)
    {
        if let Some(severity) = slot_severity {
            diags.extend(into_diagnostics(entry.rule(), rule_findings, severity));
        }
    }
    diags
}

pub(crate) fn into_diagnostics(
    rule: &dyn Rule,
    findings: Vec<Finding>,
    severity: Severity,
) -> impl Iterator<Item = LintDiagnostic> {
    let name = rule.name();
    let category = rule.category();
    findings.into_iter().map(move |finding| finding.into_diagnostic(name, category, severity))
}

/// The severity `rule` runs at, or `None` when it's off. The rule's own
/// level wins, then its category's. With neither set, only correctness rules
/// run, each at its own severity.
pub(crate) fn rule_severity(rule: &dyn Rule, config: &LintConfig) -> Option<Severity> {
    config
        .rules
        .level(rule.name())
        .or_else(|| config.categories.level(rule.category()))
        .map_or_else(
            || (rule.category() == Category::Correctness).then(|| rule.default_severity()),
            RuleLevel::severity,
        )
}
