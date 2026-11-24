use alloc::rc::Rc;
use core::cell::RefCell;

use luck_allocator::Allocator;
use luck_ast::{AST_TYPE_MAX, AstType};
use luck_diagnostics::{Diagnostic, Severity};
use luck_parser::ParseResult;
use luck_semantic::AstTypesBitset;
use luck_stdlib::{SemanticAnalysis, StdlibLibrary};
use luck_token::LintVerb;

use crate::config::{LintConfig, RuleLevel};
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::{Category, Finding, Message};
use crate::directives::Directives;
use crate::fix::FixKind;
use crate::rule::{Rule, RuleRunner};
use crate::rules::{RULES, RuleEntry};

/// How many nodes the pass hands its rules between asking whether the lint
/// was cancelled.
const NODES_PER_CANCEL_CHECK: usize = 1024;

#[expect(clippy::as_conversions, reason = "a const has no other way to widen a `u8`")]
const AST_TYPE_COUNT: usize = (AST_TYPE_MAX as usize).saturating_add(1);

thread_local! {
    /// For each type of node, the rules that run on it. Kept per thread and
    /// reused across files, so a pass allocates no buckets.
    static RULE_BUCKETS: RefCell<Box<[Vec<usize>; AST_TYPE_COUNT]>> =
        RefCell::new(Box::new([const { Vec::new() }; AST_TYPE_COUNT]));
}

/// Parses `source` into `allocator` as `stdlib`'s target and lints it,
/// knowing the globals `stdlib` declares. A diagnostic carries a fix only of
/// the kinds `fixes` holds, so an empty set builds none.
///
/// # Panics
///
/// In debug builds, as [`lint_analyzed`] does.
#[must_use]
pub fn lint(
    allocator: &Allocator,
    source: &[u8],
    stdlib: &StdlibLibrary<'_>,
    config: &LintConfig,
    fixes: FixKind,
) -> Vec<Message> {
    let parse = luck_parser::parse(allocator, source, stdlib.target.version());
    lint_parsed(&parse, stdlib, config, fixes)
}

/// [`analyze`]s `parse` against `stdlib` and lints it, building the fixes of
/// the kinds `fixes` holds.
///
/// A parse with errors yields its errors, each an E008, and no rules run, so
/// neither config nor a directive turns one off. So does a clean parse of a
/// chunk Lua refuses to compile, with the errors semantic analysis finds, such
/// as a write to a `<const>` local or more locals than a function holds.
///
/// # Panics
///
/// In debug builds, as [`lint_analyzed`] does.
#[must_use]
pub fn lint_parsed(
    parse: &ParseResult<'_>,
    stdlib: &StdlibLibrary<'_>,
    config: &LintConfig,
    fixes: FixKind,
) -> Vec<Message> {
    if !parse.errors.is_empty() {
        return error_messages(&parse.errors);
    }
    lint_analyzed(parse, &analyze(parse, stdlib, config), config, fixes, &|| false)
        .unwrap_or_default()
}

/// Lints an existing parse and its analysis, so hosts like the LSP don't
/// build either twice, building the fixes of the kinds `fixes` holds. `config`
/// must be the one `analysis` was built with.
///
/// A parse with errors, or an analysis with errors, yields them, as
/// [`lint_parsed`] does. `is_cancelled` is asked between rules and between
/// runs of nodes, and once it says so, the lint stops and returns `None`.
///
/// # Panics
///
/// In debug builds, when a rule's `node_types()` omits a type its `run` acts
/// on, or when a rule offers a fix of a kind its `fix_kind()` leaves out.
#[must_use]
pub fn lint_analyzed(
    parse: &ParseResult<'_>,
    analysis: &Analysis<'_>,
    config: &LintConfig,
    fixes: FixKind,
    is_cancelled: &dyn Fn() -> bool,
) -> Option<Vec<Message>> {
    if !parse.errors.is_empty() {
        return Some(error_messages(&parse.errors));
    }
    if !analysis.errors.is_empty() {
        return Some(error_messages(&analysis.errors));
    }
    let host = ContextHost::new(parse, analysis, config, fixes);
    let mut messages = run_rules(&Rc::new(host), is_cancelled)?;
    messages.sort_by_key(|message| message.span().start);
    Some(messages)
}

/// A parse's semantic analysis, with the compile-time errors it found, and
/// its lint directives.
#[derive(Debug)]
pub struct Analysis<'ast> {
    semantic: SemanticAnalysis<'ast>,
    errors: Vec<Diagnostic>,
    directives: Directives,
}

impl<'ast> Analysis<'ast> {
    #[must_use]
    pub const fn semantic(&self) -> &SemanticAnalysis<'ast> {
        &self.semantic
    }

    pub(crate) const fn directives(&self) -> &Directives {
        &self.directives
    }
}

/// `parse`'s analysis against `stdlib`, a library of the dialect it was
/// parsed as, with `config`'s extra globals defined.
#[must_use]
pub fn analyze<'ast>(
    parse: &'ast ParseResult<'_>,
    stdlib: &'ast StdlibLibrary<'ast>,
    config: &LintConfig,
) -> Analysis<'ast> {
    let chunk = &parse.chunk;
    let mut directives = Directives::new(&parse.comments, parse.source);
    let (mut semantic, errors) = luck_stdlib::analyze(chunk, parse.counts, stdlib);
    directives.cover_statements(semantic.semantic().nodes(), parse.source, chunk.version);
    semantic.add_extra_globals(config.extra_globals.iter().map(String::as_str));
    Analysis { semantic, errors, directives }
}

fn error_messages(errors: &[Diagnostic]) -> Vec<Message> {
    errors.iter().map(|error| Message { diagnostic: error.clone(), fix: None }).collect()
}

/// Runs the rules config turns on, or a `deny` or `warn` names.
///
/// They run together in one pass over the node table, as oxlint runs them. A
/// rule is skipped where it says it can't report, and where the file holds
/// none of the node types it runs on.
///
/// Results go back into registry order, so the caller's stable sort breaks
/// ties the same way on every run.
///
/// `None` once `is_cancelled` says so, which it's asked before each rule's
/// `run_once`, between runs of nodes, and before `unused_lint_filter`, so a
/// stopped lint never passes for a whole one.
fn run_rules(host: &Rc<ContextHost<'_>>, is_cancelled: &dyn Fn() -> bool) -> Option<Vec<Message>> {
    let (config, directives) = (host.config, host.directives);
    let severities: Vec<Option<Severity>> =
        RULES.iter().map(|entry| rule_severity(entry.rule(), config)).collect();
    let runs =
        |slot: usize| severities.get(slot).copied().flatten().is_some() || directives.enables(slot);
    let mut slots: Vec<usize> = Vec::new();
    let mut rules: Vec<&dyn RuleRunner> = Vec::new();
    let mut unused_lint_filter = None;
    #[cfg(debug_assertions)]
    let mut skipped_rules: Vec<&dyn RuleRunner> = Vec::new();
    for (slot, entry) in RULES.iter().enumerate() {
        if !runs(slot) {
            continue;
        }
        match *entry {
            RuleEntry::Rule(rule) => {
                if !rule.should_run(host) {
                    continue;
                }
                let node_types = rule.node_types();
                if node_types != AstTypesBitset::new() && !host.nodes.contains_any(node_types) {
                    #[cfg(debug_assertions)]
                    skipped_rules.push(rule);
                    continue;
                }
                slots.push(slot);
                rules.push(rule);
            }
            RuleEntry::UnusedLintFilter(rule) => unused_lint_filter = Some((slot, rule)),
        }
    }

    let findings = execute(&rules, host, is_cancelled)?;

    #[cfg(debug_assertions)]
    assert_dispatch_is_pure(&rules, &skipped_rules, &findings, host);

    let mut used = vec![false; directives.entries().len()];
    let mut messages = Vec::with_capacity(findings.iter().map(Vec::len).sum());
    for (slot, rule_findings) in slots.into_iter().zip(findings) {
        if !rule_findings.is_empty() {
            let severity = severities.get(slot).copied().flatten();
            report(slot, rule_findings, severity, directives, Some(&mut used), &mut messages);
        }
    }
    // It reads which `allow`s the other rules' diagnostics met, so it runs
    // last, as it comes last in the registry.
    if let Some((slot, rule)) = unused_lint_filter {
        if is_cancelled() {
            return None;
        }
        let rule_findings = rule.check(host, &used, runs);
        let severity = severities.get(slot).copied().flatten();
        report(slot, rule_findings, severity, directives, None, &mut messages);
    }
    Some(messages)
}

/// Each rule's findings, in the order it reported them, or `None` once
/// `is_cancelled` says so.
///
/// Each rule's `run_once` runs first, and then one pass over the node table
/// hands each node to the rules that run on its type.
pub(crate) fn execute(
    rules: &[&dyn RuleRunner],
    host: &Rc<ContextHost<'_>>,
    is_cancelled: &dyn Fn() -> bool,
) -> Option<Vec<Vec<Finding>>> {
    let contexts: Vec<LintContext<'_>> =
        rules.iter().map(|_| LintContext::new(Rc::clone(host))).collect();
    let is_finished = RULE_BUCKETS.with_borrow_mut(|buckets| {
        for bucket in buckets.iter_mut() {
            bucket.clear();
        }
        for (slot, rule) in rules.iter().enumerate() {
            for ty in rule.node_types().indices() {
                if let Some(bucket) = buckets.get_mut(ty) {
                    bucket.push(slot);
                }
            }
        }
        for (rule, context) in rules.iter().zip(&contexts) {
            if is_cancelled() {
                return false;
            }
            rule.run_once(context);
        }
        for (i, node) in host.nodes.iter().enumerate() {
            if i.is_multiple_of(NODES_PER_CANCEL_CHECK) && is_cancelled() {
                return false;
            }
            #[expect(
                clippy::indexing_slicing,
                reason = "hot loop; every bucket and rule index is in bounds by construction"
            )]
            for &slot in &buckets[bucket_of(node.kind().ty())] {
                rules[slot].run(node, &contexts[slot]);
            }
        }
        true
    });
    is_finished.then(|| contexts.into_iter().map(LintContext::into_findings).collect())
}

#[expect(clippy::as_conversions, reason = "a type's discriminant is its bucket")]
const fn bucket_of(ty: AstType) -> usize {
    ty as usize
}

/// Re-runs every rule on every node and checks that bucketed dispatch and
/// the per-file skipping changed nothing, as oxlint's debug builds do.
///
/// # Panics
///
/// On a mismatch, which means a rule's `node_types()` misses a type its `run`
/// acts on.
#[cfg(debug_assertions)]
fn assert_dispatch_is_pure(
    rules: &[&dyn RuleRunner],
    skipped_rules: &[&dyn RuleRunner],
    findings: &[Vec<Finding>],
    host: &Rc<ContextHost<'_>>,
) {
    for ((rule, bucketed), reference) in
        rules.iter().zip(findings).zip(run_on_every_node(rules, host))
    {
        assert_eq!(
            bucketed,
            &reference,
            "rule `{}`: bucketed dispatch diverged from every-node dispatch; \
             its `node_types()` declaration is missing a type",
            rule.name()
        );
    }
    for (rule, reference) in skipped_rules.iter().zip(run_on_every_node(skipped_rules, host)) {
        assert!(
            reference.is_empty(),
            "rule `{}` was skipped for this file but produces findings; \
             its `node_types()` declaration is missing a type",
            rule.name()
        );
    }
}

#[cfg(debug_assertions)]
fn run_on_every_node(rules: &[&dyn RuleRunner], host: &Rc<ContextHost<'_>>) -> Vec<Vec<Finding>> {
    rules
        .iter()
        .map(|rule| {
            let context = LintContext::new(Rc::clone(host));
            rule.run_once(&context);
            for node in host.nodes.iter() {
                rule.run(node, &context);
            }
            context.into_findings()
        })
        .collect()
}

/// Makes the findings of the rule at `slot` its messages.
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
    messages: &mut Vec<Message>,
) {
    let Some(rule) = RULES.get(slot).map(RuleEntry::rule) else {
        return;
    };
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
        let message = finding.into_message(rule, level);
        debug_assert!(
            message.fix.as_ref().is_none_or(|fix| rule.fix_kind().contains(fix.kind)),
            "rule `{}` offers a fix of a kind its `fix_kind()` leaves out: {message:?}",
            rule.name()
        );
        messages.push(message);
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

    use luck_stdlib::library_for;
    use luck_token::{LuaTarget, LuaVersion};

    use super::*;

    fn silent() -> LintConfig {
        let mut config = LintConfig::default();
        config.categories.set_level(Category::Correctness, RuleLevel::Off);
        config
    }

    #[test]
    fn a_deny_runs_its_rule_where_config_runs_none() {
        let allocator = Allocator::new();
        let source = "\
local x = nil -- luck: deny(redundant_nil_init)
local y = nil
print(x, y)";
        let parse = luck_parser::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        let silent = silent();
        let analysis = analyze(&parse, library_for(LuaTarget::Lua54), &silent);
        let diags = lint_analyzed(&parse, &analysis, &silent, FixKind::empty(), &|| false)
            .expect("nothing cancels it");
        let found: Vec<(&str, u32, Severity)> =
            diags.iter().map(|diag| (diag.code, diag.span().start, diag.severity)).collect();
        assert_eq!(found, [("redundant_nil_init", 0, Severity::Error)], "{diags:?}");
    }

    #[test]
    fn a_cancelled_lint_returns_nothing() {
        let allocator = Allocator::new();
        let source = "local x = nil -- luck: deny(redundant_nil_init)";
        let parse = luck_parser::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        let silent = silent();
        let analysis = analyze(&parse, library_for(LuaTarget::Lua54), &silent);
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
        let parse = luck_parser::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        let questions = |config: &LintConfig| {
            let analysis = analyze(&parse, library_for(LuaTarget::Lua54), config);
            let asked = AtomicUsize::new(0);
            let counted = || asked.fetch_add(1, Ordering::Relaxed) == usize::MAX;
            let whole = lint_analyzed(&parse, &analysis, config, FixKind::empty(), &counted)
                .expect("nothing cancels it");
            (asked.load(Ordering::Relaxed), whole)
        };
        let config = LintConfig::default();
        let (asked, whole) = questions(&config);
        assert!(whole.iter().any(|diag| diag.code == "unused_lint_filter"), "{whole:?}");
        let mut without_filter = LintConfig::default();
        without_filter.rules.set_level("unused_lint_filter", RuleLevel::Off);
        let (asked_without, _) = questions(&without_filter);
        assert_eq!(asked, asked_without.saturating_add(1), "one more, just before the filter");

        let analysis = analyze(&parse, library_for(LuaTarget::Lua54), &config);
        let answered = AtomicUsize::new(0);
        let cancels_last = || answered.fetch_add(1, Ordering::Relaxed).saturating_add(1) == asked;
        let stopped = lint_analyzed(&parse, &analysis, &config, FixKind::empty(), &cancels_last);
        assert_eq!(stopped, None, "diagnostics missing the unused `allow` would pass for whole");
    }

    #[test]
    fn a_rule_runs_only_where_the_file_holds_a_node_it_runs_on() {
        let allocator = Allocator::new();
        let mut config = silent();
        config.rules.set_level("compare_nan", RuleLevel::Warn);
        for (source, expected) in [("print(1)", 0), ("print(x == 0/0)", 1)] {
            let parse = luck_parser::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
            let analysis = analyze(&parse, library_for(LuaTarget::Lua54), &config);
            let diags = lint_analyzed(&parse, &analysis, &config, FixKind::empty(), &|| false)
                .expect("nothing cancels it");
            assert_eq!(diags.len(), expected, "{source}: {diags:?}");
        }
    }
}
