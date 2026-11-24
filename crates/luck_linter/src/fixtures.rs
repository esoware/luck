use luck_allocator::Allocator;
use luck_diagnostics::{Diagnostic, Severity};
use luck_stdlib::library_for;
use luck_token::{LuaTarget, LuaVersion, Span};

use crate::config::LintConfig;
use crate::context::ContextHost;
use crate::diagnostic::Message;
use crate::fix::{self, FixKind};
use crate::linter::Linter;
use crate::rules::RuleId;

pub(crate) fn run_rule(rule: RuleId, source: &str, version: LuaVersion) -> Vec<Message> {
    run_rule_with_config(rule, source, version, &LintConfig::default())
}

pub(crate) fn run_rule_roblox(rule: RuleId, source: &str) -> Vec<Message> {
    check(rule, source, LuaTarget::LuauRoblox, &LintConfig::default())
}

pub(crate) fn run_rule_with_config(
    rule: RuleId,
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
) -> Vec<Message> {
    check(rule, source, version.into(), config)
}

/// What `answer` says of the host of `source`, parsed for `target` with no
/// errors and analyzed with the default config.
pub(crate) fn with_host<T>(
    source: &str,
    target: LuaTarget,
    answer: impl FnOnce(&ContextHost<'_>) -> T,
) -> T {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, source.as_bytes(), target.version());
    assert!(parse.errors.is_empty(), "parse of {source:?}: {:?}", parse.errors);
    let analysis = Linter::new(LintConfig::default()).analyze(&parse, library_for(target));
    answer(&ContextHost::new(&parse, &analysis, FixKind::all()))
}

/// Applies one round of the messages' fixes, whatever their kind, as
/// `--fix-suggestions` does, asserting that each message carries one and
/// that the result re-parses.
pub(crate) fn fix_and_reparse(source: &str, diags: &[Message], version: LuaVersion) -> String {
    assert!(diags.iter().all(|diag| diag.fix.is_some()), "{diags:?}");
    let fixed = applied(source, diags);
    let allocator = Allocator::new();
    let reparse = luck_parser::parse(&allocator, fixed.as_bytes(), version);
    assert!(reparse.errors.is_empty(), "reparse: {:?}", reparse.errors);
    fixed
}

/// `source` after one round of the messages' fixes, whatever their kind.
pub(crate) fn applied(source: &str, diags: &[Message]) -> String {
    fix::apply(source.as_bytes(), diags, FixKind::all())
        .map_or_else(|| source.to_owned(), |fixed| String::from_utf8(fixed).unwrap())
}

/// `rule`'s messages on `source`, which it reports even where `config` turns
/// it off, at its own severity.
fn check(rule: RuleId, source: &str, target: LuaTarget, config: &LintConfig) -> Vec<Message> {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, source.as_bytes(), target.version());
    assert!(parse.errors.is_empty(), "parse: {:?}", parse.errors);
    let linter = Linter::new(config.clone());
    linter.run_rule(rule, &parse, &linter.analyze(&parse, library_for(target)))
}

/// The text each of `message`'s labels points at in `source`, in order.
pub(crate) fn labeled<'source>(message: &Message, source: &'source str) -> Vec<&'source str> {
    message
        .labels
        .iter()
        .map(|label| str::from_utf8(label.span.text(source.as_bytes())).unwrap())
        .collect()
}

/// A message with no fix, at `span`.
pub(crate) fn message(span: Span) -> Message {
    Message { diagnostic: Diagnostic::lint("test", Severity::Warning, span, "test"), fix: None }
}
