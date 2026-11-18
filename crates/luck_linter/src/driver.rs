use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_parser::ParseResult;
use luck_semantic::SemanticAnalysis;
use luck_token::{LintVerb, SourceError};
use rayon::prelude::*;

use crate::bus;
use crate::config::{LintConfig, RuleLevel};
use crate::diagnostic::{Category, Finding, LintDiagnostic, Severity};
use crate::directives::Directives;
use crate::fix::FixKind;
use crate::nodes::{NodeCollector, Nodes};
use crate::rule::{LintContext, NodeRule, Rule, WholeRule};
use crate::rules::{RULES, RuleEntry};

/// The rule a parse error's diagnostic names. No rule has the name, so
/// neither config nor a directive can turn one off.
const PARSE_ERROR_RULE: &str = "parse_error";

/// Parses `source` into `allocator` and lints it for `target`, whose stdlib
/// environment decides which globals exist. A diagnostic carries a fix only
/// of the kinds `fixes` holds, so an empty set builds none.
///
/// # Panics
///
/// In debug builds, as [`lint_analyzed`] does.
#[must_use]
pub fn lint(
    allocator: &Allocator,
    source: &str,
    target: LuaTarget,
    config: &LintConfig,
    fixes: FixKind,
) -> Vec<LintDiagnostic> {
    lint_parsed(&luck_parser::parse(allocator, source, target.lua_version()), target, config, fixes)
}

/// [`analyze`]s `parse` for `target` and lints it, building the fixes of
/// the kinds `fixes` holds. A parse with errors yields one `parse_error`
/// diagnostic per error, and no rules run.
///
/// # Panics
///
/// In debug builds, as [`lint_analyzed`] does.
#[must_use]
pub fn lint_parsed(
    parse: &ParseResult<'_>,
    target: LuaTarget,
    config: &LintConfig,
    fixes: FixKind,
) -> Vec<LintDiagnostic> {
    if !parse.errors.is_empty() {
        return parse_error_diags(parse);
    }
    lint_analyzed(parse, &analyze(parse, target, config), config, fixes, &|| false)
        .unwrap_or_default()
}

/// Lints an existing parse and its analysis, so hosts like the LSP don't
/// build either twice, building the fixes of the kinds `fixes` holds. `config`
/// must be the one `analysis` was built with.
///
/// A parse with errors yields one `parse_error` diagnostic per error, and no
/// rules run. `is_cancelled` is asked between rules and between runs of
/// nodes, and once it says so, the lint stops and returns `None`.
///
/// # Panics
///
/// In debug builds, when a rule's `node_types()` omits a type its hooks act
/// on, or when a rule offers a fix of a kind its `fix_kind()` leaves out.
#[must_use]
pub fn lint_analyzed(
    parse: &ParseResult<'_>,
    analysis: &Analysis<'_>,
    config: &LintConfig,
    fixes: FixKind,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<LintDiagnostic>> {
    if !parse.errors.is_empty() {
        return Some(parse_error_diags(parse));
    }
    let mut diags = run_rules(&LintContext::new(parse, analysis, config, fixes), is_cancelled)?;
    diags.sort_by_key(|diag| diag.span.start);
    Some(diags)
}

/// A parse's semantic analysis, the linter's table of its nodes, which
/// [`analyze`] builds in one walk, and its lint directives.
#[derive(Debug)]
pub struct Analysis<'ast> {
    semantic: SemanticAnalysis<'ast>,
    nodes: Nodes<'ast>,
    directives: Directives,
}

impl<'ast> Analysis<'ast> {
    #[must_use]
    pub const fn semantic(&self) -> &SemanticAnalysis<'ast> {
        &self.semantic
    }

    pub(crate) const fn nodes(&self) -> &Nodes<'ast> {
        &self.nodes
    }

    pub(crate) const fn directives(&self) -> &Directives {
        &self.directives
    }
}

/// `parse`'s analysis for `target`, with `config`'s extra globals defined.
///
/// Every rule reads the node table, so a file where neither config nor a
/// `deny` or `warn` runs a rule, as under the language server's default
/// config, gets an empty one and skips building it.
#[must_use]
pub fn analyze<'ast>(
    parse: &'ast ParseResult<'_>,
    target: LuaTarget,
    config: &LintConfig,
) -> Analysis<'ast> {
    let (version, environment) = (target.lua_version(), target.stdlib_environment());
    let block = &parse.block;
    let mut directives = Directives::new(&parse.comments, parse.source);
    let (mut semantic, nodes) = if RULES.iter().enumerate().any(|(slot, entry)| {
        rule_severity(entry.rule(), config).is_some() || directives.enables(slot)
    }) {
        let mut collector = NodeCollector::new(block);
        let semantic =
            luck_semantic::analyze_with(block, parse.counts, version, environment, &mut collector);
        (semantic, collector.finish())
    } else {
        (luck_semantic::analyze(block, parse.counts, version, environment), Nodes::default())
    };
    directives.cover_statements(&nodes, parse.source, version);
    semantic.add_extra_globals(config.extra_globals.iter().map(String::as_str));
    Analysis { semantic, nodes, directives }
}

fn parse_error_diags(parse: &ParseResult<'_>) -> Vec<LintDiagnostic> {
    parse.errors.iter().map(parse_error_diag).collect()
}

fn parse_error_diag(error: &SourceError) -> LintDiagnostic {
    LintDiagnostic {
        rule: PARSE_ERROR_RULE,
        category: Category::Correctness,
        severity: Severity::Error,
        message: error.message.clone().into(),
        span: error.span,
        help: None,
        fix: None,
    }
}

/// Node rules share one bus pass while whole-tree rules run beside it, on
/// the rayon pool for a file large enough to gain from it. A rule runs where
/// config turns it on, or where a `deny` or `warn` names it.
///
/// Results go back into registry order, so the caller's stable sort breaks
/// ties the way a sequential run would. A node rule is skipped when none of
/// its node types occur in this file.
///
/// `None` once `is_cancelled` says so, which it's asked before each
/// whole-tree rule, between runs of nodes, and before `unused_lint_filter`,
/// so a stopped lint never passes for a whole one.
fn run_rules(
    context: &LintContext<'_>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<LintDiagnostic>> {
    let (config, directives, nodes) = (context.config, context.directives, context.nodes);
    let severities: Vec<Option<Severity>> =
        RULES.iter().map(|entry| rule_severity(entry.rule(), config)).collect();
    let runs =
        |slot: usize| severities.get(slot).copied().flatten().is_some() || directives.enables(slot);
    let mut whole_rules: Vec<(usize, &dyn WholeRule)> = Vec::new();
    let mut node_slots: Vec<usize> = Vec::new();
    let mut node_rules: Vec<&dyn NodeRule> = Vec::new();
    let mut unused_lint_filter = None;
    #[cfg(debug_assertions)]
    let mut skipped_node_rules: Vec<&dyn NodeRule> = Vec::new();
    for (slot, entry) in RULES.iter().enumerate() {
        if !runs(slot) {
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
            RuleEntry::UnusedLintFilter(rule) => unused_lint_filter = Some((slot, rule)),
        }
    }

    let check_whole = |&(slot, rule): &(usize, &dyn WholeRule)| {
        (!is_cancelled()).then(|| (slot, rule.check(context)))
    };
    let ast_nodes = nodes.as_slice();
    let (bus_results, whole_rule_results): (_, Option<Vec<_>>) =
        if ast_nodes.len() < bus::MIN_PARALLEL_NODES {
            (
                bus::run(&node_rules, ast_nodes, context, is_cancelled),
                whole_rules.iter().map(check_whole).collect(),
            )
        } else {
            rayon::join(
                || bus::run_in_parallel(&node_rules, ast_nodes, context, is_cancelled),
                || whole_rules.par_iter().map(check_whole).collect(),
            )
        };
    let (node_results, whole_results) = (bus_results?, whole_rule_results?);

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

    let mut used = vec![false; directives.entries().len()];
    let mut diags = Vec::with_capacity(findings_by_slot.iter().map(Vec::len).sum());
    for (slot, rule_findings) in findings_by_slot.into_iter().enumerate() {
        if !rule_findings.is_empty() {
            let severity = severities.get(slot).copied().flatten();
            report(slot, rule_findings, severity, directives, Some(&mut used), &mut diags);
        }
    }
    // It reads which `allow`s the other rules' diagnostics met, so it runs
    // last, as it comes last in the registry.
    if let Some((slot, rule)) = unused_lint_filter {
        if is_cancelled() {
            return None;
        }
        let rule_findings = rule.check(context, &used, runs);
        let severity = severities.get(slot).copied().flatten();
        report(slot, rule_findings, severity, directives, None, &mut diags);
    }
    Some(diags)
}

/// Makes the findings of the rule at `slot` its diagnostics.
///
/// Each takes the level the directive that wins over it sets, or else
/// `severity`, which is `None` for a rule only a directive runs. An `allow`
/// drops a finding, and one that does is marked in `used`, when given.
pub(crate) fn report(
    slot: usize,
    mut findings: Vec<Finding>,
    severity: Option<Severity>,
    directives: &Directives,
    mut used: Option<&mut [bool]>,
    diags: &mut Vec<LintDiagnostic>,
) {
    let Some(rule) = RULES.get(slot).map(RuleEntry::rule) else {
        return;
    };
    let (name, category, fix_kind) = (rule.name(), rule.category(), rule.fix_kind());
    let mut cover = directives.cover_of(slot);
    if !cover.is_empty() && !findings.is_sorted_by_key(|finding| finding.span().start) {
        findings.sort_by_key(|finding| finding.span().start);
    }
    for finding in findings {
        let level = match cover.at(finding.span().start) {
            Some((LintVerb::Allow, entry)) => {
                if let Some(is_used) = used.as_deref_mut().and_then(|marks| marks.get_mut(entry)) {
                    *is_used = true;
                }
                continue;
            }
            Some((LintVerb::Deny, _)) => Severity::Error,
            Some((LintVerb::Warn, _)) => Severity::Warning,
            None => match severity {
                Some(level) => level,
                None => continue,
            },
        };
        let diag = finding.into_diagnostic(name, category, level);
        debug_assert!(
            diag.fix.as_ref().is_none_or(|fix| fix_kind.contains(fix.kind)),
            "rule `{name}` offers a fix of a kind its `fix_kind()` leaves out: {diag:?}"
        );
        diags.push(diag);
    }
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

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use luck_token::LuaVersion;

    use super::*;

    fn silent() -> LintConfig {
        let mut config = LintConfig::default();
        config.categories.set_level(Category::Correctness, RuleLevel::Off);
        config
    }

    #[test]
    fn a_config_that_runs_no_rule_builds_no_node_table() {
        let allocator = Allocator::new();
        let parse = luck_parser::parse(&allocator, "local x = nil print(x)", LuaVersion::Lua54);
        let silent = silent();
        let unlinted = analyze(&parse, LuaTarget::Lua54, &silent);
        assert!(unlinted.nodes().as_slice().is_empty());
        assert_eq!(
            lint_analyzed(&parse, &unlinted, &silent, FixKind::all(), &|| false),
            Some(Vec::new())
        );
        let mut one_rule = silent;
        one_rule.rules.set_level("redundant_nil_init", RuleLevel::Warn);
        let linted = analyze(&parse, LuaTarget::Lua54, &one_rule);
        assert!(!linted.nodes().as_slice().is_empty());
        let diags = lint_analyzed(&parse, &linted, &one_rule, FixKind::all(), &|| false)
            .expect("nothing cancels it");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn a_deny_runs_its_rule_where_config_runs_none() {
        let allocator = Allocator::new();
        let source = "\
local x = nil -- luck: deny(redundant_nil_init)
local y = nil
print(x, y)";
        let parse = luck_parser::parse(&allocator, source, LuaVersion::Lua54);
        let silent = silent();
        let analysis = analyze(&parse, LuaTarget::Lua54, &silent);
        assert!(!analysis.nodes().as_slice().is_empty());
        let diags = lint_analyzed(&parse, &analysis, &silent, FixKind::empty(), &|| false)
            .expect("nothing cancels it");
        let found: Vec<(&str, u32, Severity)> =
            diags.iter().map(|diag| (diag.rule, diag.span.start, diag.severity)).collect();
        assert_eq!(found, [("redundant_nil_init", 0, Severity::Error)], "{diags:?}");
    }

    #[test]
    fn a_cancelled_lint_returns_nothing() {
        let allocator = Allocator::new();
        let source = "local x = nil -- luck: deny(redundant_nil_init)";
        let parse = luck_parser::parse(&allocator, source, LuaVersion::Lua54);
        let silent = silent();
        let analysis = analyze(&parse, LuaTarget::Lua54, &silent);
        let cancelled = lint_analyzed(&parse, &analysis, &silent, FixKind::empty(), &|| true);
        assert_eq!(cancelled, None, "a rule only a directive runs stops too");
    }

    #[test]
    fn a_lint_cancelled_before_unused_lint_filter_returns_nothing() {
        let allocator = Allocator::new();
        let source = "\
-- luck: allow(undefined_variable)
local x = 1
print(x)";
        let parse = luck_parser::parse(&allocator, source, LuaVersion::Lua54);
        let questions = |config: &LintConfig| {
            let analysis = analyze(&parse, LuaTarget::Lua54, config);
            let asked = AtomicUsize::new(0);
            let counted = || asked.fetch_add(1, Ordering::Relaxed) == usize::MAX;
            let whole = lint_analyzed(&parse, &analysis, config, FixKind::empty(), &counted)
                .expect("nothing cancels it");
            (asked.load(Ordering::Relaxed), whole)
        };
        let config = LintConfig::default();
        let (asked, whole) = questions(&config);
        assert!(whole.iter().any(|diag| diag.rule == "unused_lint_filter"), "{whole:?}");
        let mut without_filter = LintConfig::default();
        without_filter.rules.set_level("unused_lint_filter", RuleLevel::Off);
        let (asked_without, _) = questions(&without_filter);
        assert_eq!(asked, asked_without.saturating_add(1), "one more, just before the filter");

        let analysis = analyze(&parse, LuaTarget::Lua54, &config);
        let answered = AtomicUsize::new(0);
        let cancels_last = || answered.fetch_add(1, Ordering::Relaxed).saturating_add(1) == asked;
        let stopped = lint_analyzed(&parse, &analysis, &config, FixKind::empty(), &cancels_last);
        assert_eq!(stopped, None, "diagnostics missing the unused `allow` would pass for whole");
    }
}
