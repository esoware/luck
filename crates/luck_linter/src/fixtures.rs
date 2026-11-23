use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_diagnostics::{Diagnostic, Severity};
use luck_token::{LuaVersion, Span};

use crate::config::LintConfig;
use crate::diagnostic::Message;
use crate::fix::{self, FixKind};
use crate::rule::{LintContext, Rule};
use crate::rules::{RULES, RuleEntry};
use crate::{bus, driver};

pub(crate) fn run_rule(rule: &dyn Rule, source: &str, version: LuaVersion) -> Vec<Message> {
    run_rule_with_config(rule, source, version, &LintConfig::default())
}

pub(crate) fn run_rule_roblox(rule: &dyn Rule, source: &str) -> Vec<Message> {
    check(rule, source, LuaTarget::LuauRoblox, &LintConfig::default())
}

pub(crate) fn run_rule_with_config(
    rule: &dyn Rule,
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
) -> Vec<Message> {
    check(rule, source, standalone(version), config)
}

/// Applies one round of the messages' fixes, whatever their kind, as
/// `--fix-suggestions` does, asserting that each message carries one and
/// that the result re-parses.
pub(crate) fn fix_and_reparse(source: &str, diags: &[Message], version: LuaVersion) -> String {
    assert!(diags.iter().all(|diag| diag.fix.is_some()), "{diags:?}");
    let fixed = applied(source, diags);
    let allocator = Allocator::new();
    let reparse = luck_parser::parse(&allocator, &fixed, version);
    assert!(reparse.errors.is_empty(), "reparse: {:?}", reparse.errors);
    fixed
}

/// `source` after one round of the messages' fixes, whatever their kind.
pub(crate) fn applied(source: &str, diags: &[Message]) -> String {
    fix::apply(source, diags, FixKind::all()).unwrap_or_else(|| source.to_owned())
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

fn check(rule: &dyn Rule, source: &str, target: LuaTarget, config: &LintConfig) -> Vec<Message> {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, source, target.lua_version());
    assert!(parse.errors.is_empty(), "parse: {:?}", parse.errors);
    let analysis = driver::analyze(&parse, target.stdlib_environment(), config);
    let context = LintContext::new(&parse, &analysis, config, FixKind::all());
    let slot = RULES
        .iter()
        .position(|entry| entry.rule().name() == rule.name())
        .expect("only registered rules are tested");
    let findings = match RULES[slot] {
        RuleEntry::Whole(whole_rule) => whole_rule.check(&context),
        RuleEntry::Node(node_rule) => {
            bus::run(&[node_rule], analysis.nodes().as_slice(), &context, &|| false)
                .and_then(|mut findings| findings.pop())
                .unwrap_or_default()
        }
        // It reads what the others reported, so the whole lint runs.
        RuleEntry::UnusedLintFilter(_) => {
            return driver::lint_analyzed(&parse, &analysis, config, FixKind::all(), &|| false)
                .unwrap_or_default()
                .into_iter()
                .filter(|diag| diag.code == rule.name())
                .collect();
        }
    };
    // A rule the config leaves off still runs here, at its own severity.
    let severity = driver::rule_severity(rule, config).unwrap_or_else(|| rule.default_severity());
    let mut diags = Vec::new();
    driver::report(slot, findings, Some(severity), analysis.directives(), None, &mut diags);
    diags
}

/// The text each of `message`'s labels points at in `source`, in order.
pub(crate) fn labeled<'source>(message: &Message, source: &'source str) -> Vec<&'source str> {
    message.labels.iter().map(|label| label.span.text(source)).collect()
}

/// A message with no fix, at `span`.
pub(crate) fn message(span: Span) -> Message {
    Message { diagnostic: Diagnostic::lint("test", Severity::Warning, span, "test"), fix: None }
}
