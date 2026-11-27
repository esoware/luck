use std::cell::RefCell;

use luck_ast::AST_TYPE_MAX;
use luck_diagnostics::{Diagnostic, Severity};
use luck_parser::ParseResult;
use luck_semantic::AstTypesBitset;
use luck_stdlib::{SemanticAnalysis, StdlibLibrary};
use luck_token::LintVerb;

use crate::config::LintConfig;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::{Finding, Message};
use crate::directives::Directives;
use crate::fix::FixKind;
use crate::rules::{RuleEnum, RuleId, RuleTable, UnusedLintFilter};
use crate::timing::{RuleTimings, Spent};

/// How many nodes the pass hands its rules between asking whether the lint
/// was cancelled.
const NODES_PER_CANCEL_CHECK: usize = 1024;

const AST_TYPE_COUNT: usize = AST_TYPE_MAX as usize + 1;

thread_local! {
    /// For each type of node, the rules that run on it. Kept per thread and
    /// reused across files, so a pass allocates no buckets.
    static RULE_BUCKETS: RefCell<Box<[Vec<usize>; AST_TYPE_COUNT]>> =
        RefCell::new(Box::new([const { Vec::new() }; AST_TYPE_COUNT]));
}

/// A `lint` section compiled once, for every file it lints.
///
/// It holds each rule built from its options, and the severity it runs at,
/// which a file reads by rule id, as `oxc_linter::Linter` reads its config
/// store's resolved state.
#[derive(Debug)]
pub struct Linter {
    config: LintConfig,
    rules: RuleTable<RuleEnum>,
    /// `None` for a rule config turns off, which only a `deny` or `warn`
    /// runs.
    severities: RuleTable<Option<Severity>>,
}

impl Linter {
    pub fn new(config: LintConfig) -> Self {
        let rules = config.rules.options().configured();
        let severities = RuleTable::from_fn(|rule| config.severity(rule));
        Self { config, rules, severities }
    }

    /// The settings it was built from.
    pub const fn config(&self) -> &LintConfig {
        &self.config
    }

    /// `parse`'s analysis against `stdlib`, with the config's extra globals
    /// defined.
    pub fn analyze<'ast>(
        &self,
        parse: &'ast ParseResult<'ast>,
        stdlib: &'ast StdlibLibrary<'ast>,
    ) -> Analysis<'ast> {
        let (mut semantic, errors) = luck_stdlib::analyze(&parse.chunk, parse.counts, stdlib);
        let directives = Directives::new(parse, semantic.semantic().nodes());
        semantic.add_extra_globals(self.config.extra_globals.iter().map(String::as_str));
        Analysis { parse, semantic, errors, directives }
    }

    /// Lints `parse`, knowing the globals `stdlib` declares, and builds the
    /// fixes of the kinds `fixes` holds, so an empty set builds none. Each
    /// rule's time goes into `timings` when given, and a lint that times
    /// nothing pays nothing for it.
    ///
    /// A parse with errors yields its errors, each an E008, and no rules run,
    /// so neither config nor a directive turns one off. So does a clean parse
    /// of a chunk Luau refuses to compile, with the errors semantic analysis
    /// finds, such as a write to a `const` local. A definition file's parse,
    /// which holds no code for a rule to read, yields its errors alone.
    ///
    /// # Panics
    ///
    /// In debug builds, as [`Self::run_analyzed`] does.
    pub fn run(
        &self,
        parse: &ParseResult<'_>,
        stdlib: &StdlibLibrary<'_>,
        fixes: FixKind,
        timings: Option<&RuleTimings>,
    ) -> Vec<Message> {
        if parse.is_definition_file || !parse.errors.is_empty() {
            return error_messages(&parse.errors);
        }
        let analysis = self.analyze(parse, stdlib);
        self.run_analyzed(&analysis, fixes, timings, &|| false).expect("nothing cancels the lint")
    }

    /// [`Self::run`] on the analysis [`Self::analyze`] built of a parse,
    /// which a host like the language server keeps for its providers.
    ///
    /// `is_cancelled` is asked between rules and between runs of nodes, and
    /// once it says so, the lint stops and returns `None`.
    ///
    /// # Panics
    ///
    /// In debug builds, when a rule's `NODE_TYPES` omits a type its `run`
    /// acts on, or when a rule offers a fix of a kind its `FIX` leaves out.
    pub fn run_analyzed(
        &self,
        analysis: &Analysis<'_>,
        fixes: FixKind,
        timings: Option<&RuleTimings>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<Vec<Message>> {
        let parse = analysis.parse;
        if parse.is_definition_file || !parse.errors.is_empty() {
            return Some(error_messages(&parse.errors));
        }
        if !analysis.errors.is_empty() {
            return Some(error_messages(&analysis.errors));
        }
        let host = ContextHost::new(analysis, fixes);
        let mut messages = self.run_rules(&host, timings, is_cancelled)?;
        messages.sort_by_key(|message| message.span().start);
        Some(messages)
    }

    /// Runs the rules config turns on, or a `deny` or `warn` names.
    ///
    /// They run together in one pass over the node table, as oxlint runs
    /// them. A rule is skipped where it says it can't report, and where the
    /// file holds none of the node types it runs on.
    ///
    /// Results go back into [`RuleId::ALL`]'s order, so the caller's stable
    /// sort breaks ties the same way on every run.
    ///
    /// `None` once `is_cancelled` says so, which it's asked before each
    /// rule's `run_once`, between runs of nodes, and before
    /// `unused_lint_filter`, so a stopped lint never passes for a whole one.
    fn run_rules(
        &self,
        host: &ContextHost<'_>,
        timings: Option<&RuleTimings>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<Vec<Message>> {
        let directives = host.directives;
        let runs = |rule: RuleId| self.severities[rule].is_some() || directives.enables(rule);
        let mut rules: Vec<&RuleEnum> = Vec::new();
        #[cfg(debug_assertions)]
        let mut skipped_rules: Vec<&RuleEnum> = Vec::new();
        // `unused_lint_filter` reads which `allow`s the other rules'
        // diagnostics met, so it runs apart, after them.
        let pass_rules =
            RuleId::ALL.into_iter().filter(|&rule| rule != RuleId::UnusedLintFilter && runs(rule));
        for rule in pass_rules.map(|rule| &self.rules[rule]) {
            if !rule.should_run(host) {
                continue;
            }
            let node_types = rule.node_types();
            if node_types != AstTypesBitset::new() && !host.nodes.contains_any(node_types) {
                #[cfg(debug_assertions)]
                skipped_rules.push(rule);
                continue;
            }
            rules.push(rule);
        }

        let findings = match timings {
            Some(run_timings) => {
                let mut spent = vec![Spent::default(); rules.len()];
                let findings = execute::<true>(&rules, host, &mut spent, is_cancelled);
                for (rule, rule_spent) in rules.iter().zip(spent) {
                    run_timings.add(rule.id(), rule_spent);
                }
                findings
            }
            None => execute::<false>(&rules, host, &mut [], is_cancelled),
        }?;

        #[cfg(debug_assertions)]
        assert_dispatch_is_pure(&rules, &skipped_rules, &findings, host);

        let mut used = vec![false; directives.entries().len()];
        let mut messages = Vec::with_capacity(findings.iter().map(Vec::len).sum());
        for (rule, rule_findings) in rules.into_iter().zip(findings) {
            if !rule_findings.is_empty() {
                let id = rule.id();
                let severity = self.severities[id];
                report(id, rule_findings, severity, directives, &mut used, &mut messages);
            }
        }
        let id = RuleId::UnusedLintFilter;
        if runs(id) {
            if is_cancelled() {
                return None;
            }
            let check = || UnusedLintFilter::check(host, &used, runs);
            let rule_findings = match timings {
                Some(run_timings) => {
                    let mut spent = Spent::default();
                    let rule_findings = spent.time(check);
                    run_timings.add(id, spent);
                    rule_findings
                }
                None => check(),
            };
            report(id, rule_findings, self.severities[id], directives, &mut used, &mut messages);
        }
        Some(messages)
    }
}

/// A parse, its semantic analysis, with the compile-time errors it found,
/// and its lint directives.
#[derive(Debug)]
pub struct Analysis<'ast> {
    parse: &'ast ParseResult<'ast>,
    semantic: SemanticAnalysis<'ast>,
    errors: Vec<Diagnostic>,
    directives: Directives,
}

impl<'ast> Analysis<'ast> {
    pub const fn semantic(&self) -> &SemanticAnalysis<'ast> {
        &self.semantic
    }

    pub(crate) const fn parse(&self) -> &'ast ParseResult<'ast> {
        self.parse
    }

    pub(crate) const fn directives(&self) -> &Directives {
        &self.directives
    }
}

fn error_messages(errors: &[Diagnostic]) -> Vec<Message> {
    errors.iter().map(|error| Message { diagnostic: error.clone(), fix: None }).collect()
}

/// Each rule's findings, in the order it reported them, or `None` once
/// `is_cancelled` says so.
///
/// Each rule's `run_once` runs first, and then one pass over the node table
/// hands each node to the rules that run on its type. When `TIMED`, each
/// call goes into the rule's entry of `spent`, as oxlint's `TIMINGS` pass
/// times each, and else nothing is timed.
fn execute<const TIMED: bool>(
    rules: &[&RuleEnum],
    host: &ContextHost<'_>,
    spent: &mut [Spent],
    is_cancelled: &dyn Fn() -> bool,
) -> Option<Vec<Vec<Finding>>> {
    let contexts: Vec<LintContext<'_, '_>> = rules.iter().map(|_| LintContext::new(host)).collect();
    let is_finished = RULE_BUCKETS.with_borrow_mut(|buckets| {
        for bucket in buckets.iter_mut() {
            bucket.clear();
        }
        for (slot, rule) in rules.iter().enumerate() {
            for ty in rule.node_types().indices() {
                buckets[ty].push(slot);
            }
        }
        for (slot, (rule, context)) in rules.iter().zip(&contexts).enumerate() {
            if is_cancelled() {
                return false;
            }
            if TIMED {
                spent[slot].time(|| rule.run_once(context));
            } else {
                rule.run_once(context);
            }
        }
        for (i, node) in host.nodes.iter().enumerate() {
            if i.is_multiple_of(NODES_PER_CANCEL_CHECK) && is_cancelled() {
                return false;
            }
            for &slot in &buckets[node.kind().ty() as usize] {
                let (rule, context) = (rules[slot], &contexts[slot]);
                if TIMED {
                    spent[slot].time(|| rule.run(node, context));
                } else {
                    rule.run(node, context);
                }
            }
        }
        true
    });
    is_finished.then(|| contexts.into_iter().map(LintContext::into_findings).collect())
}

/// Re-runs every rule on every node and checks that bucketed dispatch and
/// the per-file skipping changed nothing, as oxlint's debug builds do.
#[cfg(debug_assertions)]
fn assert_dispatch_is_pure(
    rules: &[&RuleEnum],
    skipped_rules: &[&RuleEnum],
    findings: &[Vec<Finding>],
    host: &ContextHost<'_>,
) {
    for ((rule, bucketed), reference) in
        rules.iter().zip(findings).zip(run_on_every_node(rules, host))
    {
        assert_eq!(
            bucketed,
            &reference,
            "rule `{}`: bucketed dispatch diverged from every-node dispatch; \
             its `NODE_TYPES` is missing a type",
            rule.id().name()
        );
    }
    for (rule, reference) in skipped_rules.iter().zip(run_on_every_node(skipped_rules, host)) {
        assert!(
            reference.is_empty(),
            "rule `{}` was skipped for this file but produces findings; \
             its `NODE_TYPES` is missing a type",
            rule.id().name()
        );
    }
}

#[cfg(debug_assertions)]
fn run_on_every_node(rules: &[&RuleEnum], host: &ContextHost<'_>) -> Vec<Vec<Finding>> {
    rules
        .iter()
        .map(|rule| {
            let context = LintContext::new(host);
            rule.run_once(&context);
            for node in host.nodes.iter() {
                rule.run(node, &context);
            }
            context.into_findings()
        })
        .collect()
}

/// Makes `rule`'s findings its messages.
///
/// Each takes the level the directive that wins over it sets, or else
/// `severity`, which is `None` for a rule only a directive runs. An `allow`
/// drops a finding, and its entry is marked in `used`.
fn report(
    rule: RuleId,
    mut findings: Vec<Finding>,
    severity: Option<Severity>,
    directives: &Directives,
    used: &mut [bool],
    messages: &mut Vec<Message>,
) {
    let mut cover = directives.cover_of(rule);
    if !cover.is_empty() && !findings.is_sorted_by_key(|finding| finding.span().start) {
        findings.sort_by_key(|finding| finding.span().start);
    }
    for finding in findings {
        let level = match cover.at(finding.span().start) {
            Some((LintVerb::Allow, entry)) => {
                used[entry] = true;
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
            "rule `{}` offers a fix of a kind its `FIX` leaves out: {message:?}",
            rule.name()
        );
        messages.push(message);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use luck_allocator::Allocator;
    use luck_stdlib::luau_library;

    use super::*;
    use crate::config::RuleLevel;
    use crate::rule::Category;

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
        let parse = luck_parser::parse(&allocator, source.as_bytes());
        let linter = Linter::new(silent());
        let analysis = linter.analyze(&parse, luau_library());
        let diags = linter
            .run_analyzed(&analysis, FixKind::empty(), None, &|| false)
            .expect("nothing cancels it");
        let found: Vec<(&str, u32, Severity)> =
            diags.iter().map(|diag| (diag.code, diag.span().start, diag.severity)).collect();
        assert_eq!(found, [("redundant_nil_init", 0, Severity::Error)], "{diags:?}");
    }

    #[test]
    fn a_cancelled_lint_returns_nothing() {
        let allocator = Allocator::new();
        let source = "local x = nil -- luck: deny(redundant_nil_init)";
        let parse = luck_parser::parse(&allocator, source.as_bytes());
        let linter = Linter::new(silent());
        let analysis = linter.analyze(&parse, luau_library());
        let cancelled = linter.run_analyzed(&analysis, FixKind::empty(), None, &|| true);
        assert_eq!(cancelled, None, "a rule only a directive runs stops too");
    }

    #[test]
    fn a_lint_cancelled_before_unused_lint_filter_returns_nothing() {
        let allocator = Allocator::new();
        let source = "\
-- luck: allow(undefined_variable)
local x = 1
print(x)";
        let parse = luck_parser::parse(&allocator, source.as_bytes());
        let questions = |linter: &Linter| {
            let analysis = linter.analyze(&parse, luau_library());
            let asked = AtomicUsize::new(0);
            let counted = || asked.fetch_add(1, Ordering::Relaxed) == usize::MAX;
            let whole = linter
                .run_analyzed(&analysis, FixKind::empty(), None, &counted)
                .expect("nothing cancels it");
            (asked.load(Ordering::Relaxed), whole)
        };
        let linter = Linter::new(LintConfig::default());
        let (asked, whole) = questions(&linter);
        assert!(whole.iter().any(|diag| diag.code == "unused_lint_filter"), "{whole:?}");
        let mut without_filter = LintConfig::default();
        without_filter.rules.set_level(RuleId::UnusedLintFilter, RuleLevel::Off);
        let (asked_without, _) = questions(&Linter::new(without_filter));
        assert_eq!(asked, asked_without + 1, "one more, just before the filter");

        let analysis = linter.analyze(&parse, luau_library());
        let answered = AtomicUsize::new(0);
        let cancels_last = || answered.fetch_add(1, Ordering::Relaxed) + 1 == asked;
        let stopped = linter.run_analyzed(&analysis, FixKind::empty(), None, &cancels_last);
        assert_eq!(stopped, None, "diagnostics missing the unused `allow` would pass for whole");
    }

    #[test]
    fn a_rule_runs_only_where_the_file_holds_a_node_it_runs_on() {
        let allocator = Allocator::new();
        let mut config = silent();
        config.rules.set_level(RuleId::CompareNan, RuleLevel::Warn);
        let linter = Linter::new(config);
        for (source, expected) in [("print(1)", 0), ("print(x == 0/0)", 1)] {
            let parse = luck_parser::parse(&allocator, source.as_bytes());
            let diags = linter.run(&parse, luau_library(), FixKind::empty(), None);
            assert_eq!(diags.len(), expected, "{source}: {diags:?}");
        }
    }

    #[test]
    fn a_timed_lint_counts_each_call_of_each_rule_that_runs() {
        let allocator = Allocator::new();
        let mut config = silent();
        config.rules.set_level(RuleId::CompareNan, RuleLevel::Warn);
        config.rules.set_level(RuleId::UndefinedVariable, RuleLevel::Warn);
        let linter = Linter::new(config);
        let source = "print(x == 0/0, x == 1)";
        let parse = luck_parser::parse(&allocator, source.as_bytes());
        let timings = RuleTimings::new();
        let diags = linter.run(&parse, luau_library(), FixKind::empty(), Some(&timings));
        assert_eq!(diags.len(), 3, "{diags:?}");
        let mut calls: Vec<(&str, u64)> =
            timings.sorted().iter().map(|timing| (timing.rule.name(), timing.calls)).collect();
        calls.sort_unstable();
        assert_eq!(
            calls,
            [("compare_nan", 4), ("undefined_variable", 1)],
            "`compare_nan` runs once for the file and once on each of its three operator runs"
        );
    }
}
