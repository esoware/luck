use std::fmt::{self, Write as _};
use std::iter;
use std::ops::Range;
use std::slice;

use luck_allocator::Allocator;
use luck_diagnostics::{Diagnostic, Severity, Tag};
use luck_stdlib::library_for;
use luck_token::{LuaTarget, LuaVersion, Span, line_breaks};
use serde_json::{Value, json};

use crate::config::LintConfig;
use crate::context::ContextHost;
use crate::diagnostic::Message;
use crate::fix::{self, FixKind};
use crate::linter::Linter;
use crate::rules::RuleId;

/// A source a rule's test lints, as oxlint's `TestCase` is.
///
/// It's parsed as its dialect, Lua 5.4 for a plain source, and linted with
/// the rule's options and the rest of a `luck.json` `lint` section, each
/// JSON, as oxlint's holds a rule's config and the rest of `.oxlintrc.json`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TestCase<'case> {
    source: &'case str,
    target: LuaTarget,
    options: Option<&'case str>,
    lint: Option<&'case str>,
}

impl<'case> From<&'case str> for TestCase<'case> {
    fn from(source: &'case str) -> Self {
        (source, LuaTarget::Lua54).into()
    }
}

impl<'case> From<(&'case str, LuaTarget)> for TestCase<'case> {
    fn from((source, target): (&'case str, LuaTarget)) -> Self {
        (source, target, None).into()
    }
}

impl<'case> From<(&'case str, LuaTarget, Option<&'case str>)> for TestCase<'case> {
    fn from((source, target, options): (&'case str, LuaTarget, Option<&'case str>)) -> Self {
        (source, target, options, None).into()
    }
}

type FullCase<'case> = (&'case str, LuaTarget, Option<&'case str>, Option<&'case str>);

impl<'case> From<FullCase<'case>> for TestCase<'case> {
    fn from((source, target, options, lint): FullCase<'case>) -> Self {
        Self { source, target, options, lint }
    }
}

impl fmt::Display for TestCase<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} case `{}`", self.target, self.source)
    }
}

/// Tests one rule on the cases that must find nothing and those that must
/// find something, as oxlint's `Tester` does.
///
/// Every finding of the second, with its labels, help, and the source after
/// its fix, goes into the rule's snapshot. Each fix, and one round of a
/// case's fixes together, has to change the source and leave it parsing,
/// and each kind of fix the rule declares has to show in some case.
///
/// The rule runs at its default severity beside the default rules, since
/// `unused_lint_filter` reads what the others report, and only its own
/// messages count.
#[derive(Debug)]
pub(crate) struct Tester<'case> {
    rule: RuleId,
    pass: Vec<TestCase<'case>>,
    fail: Vec<TestCase<'case>>,
}

impl<'case> Tester<'case> {
    pub(crate) fn new<Case>(rule: RuleId, pass: &[Case], fail: &[Case]) -> Self
    where
        Case: Copy + Into<TestCase<'case>>,
    {
        let cases = |list: &[Case]| list.iter().map(|&case| case.into()).collect();
        Self { rule, pass: cases(pass), fail: cases(fail) }
    }

    pub(crate) fn test_and_snapshot(&self) {
        let mut problems = Vec::new();
        for case in &self.pass {
            let messages = self.lint(case);
            if !messages.is_empty() {
                let found = rendered(case, &messages, &mut Vec::new());
                problems.push(format!("{case} should find nothing, but found:\n{found}"));
            }
        }
        let mut snapshot = String::new();
        let mut shown_kinds = FixKind::empty();
        for case in &self.fail {
            let messages = self.lint(case);
            if messages.is_empty() {
                problems.push(format!("{case} should find something, but found nothing"));
                continue;
            }
            for fix in messages.iter().filter_map(|message| message.fix.as_ref()) {
                shown_kinds |= fix.kind;
            }
            if !snapshot.is_empty() {
                snapshot.push('\n');
            }
            snapshot.push_str(&rendered(case, &messages, &mut problems));
        }
        let unshown_kinds = self.rule.fix_kind().difference(shown_kinds);
        if !unshown_kinds.is_empty() {
            let names: Vec<&str> = unshown_kinds.iter_names().map(|(name, _)| name).collect();
            problems.push(format!("no failing case shows a fix of kind {}", names.join(", ")));
        }
        assert!(problems.is_empty(), "`{}`:\n\n{}", self.rule.name(), problems.join("\n\n"));
        insta::with_settings!({ prepend_module_to_snapshot => false, omit_expression => true }, {
            insta::assert_snapshot!(self.rule.name(), snapshot);
        });
    }

    /// The rule's messages on `case`, every kind of fix built.
    fn lint(&self, case: &TestCase<'_>) -> Vec<Message> {
        let level = match self.rule.default_severity() {
            Severity::Error => "error",
            Severity::Warning => "warn",
        };
        let parsed = |text: &str| -> Value {
            serde_json::from_str(text).unwrap_or_else(|e| panic!("{case}: {e}"))
        };
        let mut lint = case.lint.map_or_else(|| json!({}), parsed);
        lint["rules"][self.rule.name()] =
            case.options.map_or_else(|| json!(level), |options| json!([level, parsed(options)]));
        let config: LintConfig =
            serde_json::from_value(lint).unwrap_or_else(|e| panic!("{case}: {e}"));
        let allocator = Allocator::new();
        let parse = luck_parser::parse(&allocator, case.source.as_bytes(), case.target.version());
        assert!(parse.errors.is_empty(), "{case} doesn't parse: {:?}", parse.errors);
        let mut messages =
            Linter::new(config).run(&parse, library_for(case.target), FixKind::all(), None);
        assert!(
            messages.iter().all(|message| RuleId::from_name(message.code).is_some()),
            "{case} doesn't compile: {messages:?}"
        );
        messages.retain(|message| message.code == self.rule.name());
        messages
    }
}

/// Each of `messages`, found in `case`, as the snapshot shows it.
///
/// One round of their fixes together follows when more than one has a fix.
/// A fix that changes nothing or breaks the syntax goes into `problems`
/// instead.
fn rendered(case: &TestCase<'_>, messages: &[Message], problems: &mut Vec<String>) -> String {
    let version = case.target.version();
    let lines = Lines::new(case.source, version);
    let mut text = String::new();
    for message in messages {
        if !text.is_empty() {
            text.push('\n');
        }
        render_message(&mut text, &lines, case.target, message);
        let Some(fix) = &message.fix else {
            continue;
        };
        match fixed(case.source, slice::from_ref(message), version) {
            Ok(fixed_source) => {
                let kind = if fix.kind == FixKind::Fix { "fix" } else { "suggestion" };
                writeln!(text, "{kind}: {}", fix.message).unwrap();
                render_source(&mut text, &fixed_source, version);
            }
            Err(problem) => problems.push(format!("{case}: `{}` {problem}", fix.message)),
        }
    }
    if messages.iter().filter(|message| message.fix.is_some()).nth(1).is_some() {
        match fixed(case.source, messages, version) {
            Ok(fixed_source) => {
                text.push_str("\nall fixes:\n");
                render_source(&mut text, &fixed_source, version);
            }
            Err(problem) => problems.push(format!("{case}: its fixes together {problem}")),
        }
    }
    text
}

/// `source` after one round of the fixes of `messages`.
fn fixed(source: &str, messages: &[Message], version: LuaVersion) -> Result<String, String> {
    let Some(fixed_bytes) = fix::apply(source.as_bytes(), messages, FixKind::all()) else {
        return Err("changes nothing".to_owned());
    };
    let fixed_source = String::from_utf8(fixed_bytes).map_err(|e| e.to_string())?;
    let allocator = Allocator::new();
    let reparse = luck_parser::parse(&allocator, fixed_source.as_bytes(), version);
    match reparse.errors.first() {
        Some(error) => Err(format!("breaks the syntax, as in `{fixed_source}`: {error}")),
        None => Ok(fixed_source),
    }
}

/// A place a message marks: its span with `^`, or a label's with `-` and
/// what the label says.
type Mark<'message> = (Span, char, Option<&'message str>);

/// The message, where it is, the lines its span and labels touch, each
/// marked under the part it covers, with a line of context around them, as
/// miette shows oxlint's, and its tag, help, and note.
fn render_message(text: &mut String, lines: &Lines<'_>, target: LuaTarget, message: &Message) {
    writeln!(text, "{}: {}", message.severity.name(), message.message).unwrap();
    let marks: Vec<Mark<'_>> = iter::once((message.span(), '^', None))
        .chain(message.labels.iter().map(|label| (label.span, '-', Some(&*label.message))))
        .collect();
    let mut shown_lines: Vec<usize> = marks
        .iter()
        .flat_map(|&(span, ..)| [lines.start(span).0, lines.end(span).0])
        .flat_map(|marked| marked.saturating_sub(1)..=marked + 1)
        .filter(|&line| line < lines.ranges.len())
        .collect();
    shown_lines.sort_unstable();
    shown_lines.dedup();
    let gaps_of_one: Vec<usize> = shown_lines
        .windows(2)
        .filter_map(|pair| (pair[1] == pair[0] + 2).then_some(pair[0] + 1))
        .collect();
    shown_lines.extend(gaps_of_one);
    shown_lines.sort_unstable();
    let width = shown_lines.last().map_or(1, |&last| (last + 1).to_string().len());
    let (line, column) = lines.start(message.span());
    let column_chars = lines.text(line)[..column].chars().count();
    let (line_number, column_number) = (line + 1, column_chars + 1);
    writeln!(text, "{:width$}--> {target}:{line_number}:{column_number}", "").unwrap();
    let mut last_shown = None;
    for &shown in &shown_lines {
        if last_shown.is_some_and(|last| shown > last + 1) {
            text.push_str("...\n");
        }
        last_shown = Some(shown);
        write!(text, "{:>width$} |", shown + 1).unwrap();
        push_shown_line(text, lines.text(shown));
        for &mark in &marks {
            render_mark(text, lines, shown, width, mark);
        }
    }
    for (heading, extra) in [
        ("tag", message.tag.map(Tag::name)),
        ("help", message.help.as_deref()),
        ("note", message.note.as_deref()),
    ] {
        if let Some(extra_text) = extra {
            writeln!(text, "{heading}: {extra_text}").unwrap();
        }
    }
}

/// The marks under the part of line `shown` that `mark` covers, if it covers
/// any, and what a label says on its last line.
fn render_mark(text: &mut String, lines: &Lines<'_>, shown: usize, width: usize, mark: Mark<'_>) {
    let (span, mark_char, label) = mark;
    let line_text = lines.text(shown);
    let ((start_line, start), (end_line, end)) = (lines.start(span), lines.end(span));
    let (from, to) = match (start_line == shown, end_line == shown) {
        (true, true) => (start, end),
        (true, false) => (start, line_text.len()),
        (false, true) => (0, end),
        (false, false) => return,
    };
    write!(text, "{:width$} | ", "").unwrap();
    for c in line_text[..from].chars() {
        if c == '\t' {
            text.push('\t');
        } else {
            text.extend(iter::repeat_n(' ', shown_width(c)));
        }
    }
    let covered = &line_text[from..to];
    text.extend(iter::repeat_n(mark_char, covered.chars().map(shown_width).sum::<usize>().max(1)));
    match label {
        Some(said) if end_line == shown => writeln!(text, " {said}").unwrap(),
        Some(_) | None => text.push('\n'),
    }
}

/// Every line of `source`, behind a bar that sets it apart from the lines a
/// message marks.
fn render_source(text: &mut String, source: &str, version: LuaVersion) {
    let lines = Lines::new(source, version);
    for line in 0..lines.ranges.len() {
        text.push_str("  |");
        push_shown_line(text, lines.text(line));
    }
}

/// ` line` and a line break, or just the line break for an empty line, so no
/// line ends in a space the source doesn't have.
fn push_shown_line(text: &mut String, line: &str) {
    if !line.is_empty() {
        text.push(' ');
    }
    for c in line.chars() {
        if is_escaped(c) {
            text.extend(c.escape_default());
        } else {
            text.push(c);
        }
    }
    text.push('\n');
}

/// Whether `c` shows escaped in a snapshot, as a control character other
/// than a tab does, which would break the line or vanish.
fn is_escaped(c: char) -> bool {
    c.is_control() && c != '\t'
}

fn shown_width(c: char) -> usize {
    if is_escaped(c) { c.escape_default().count() } else { 1 }
}

/// A source split into lines where its dialect breaks them.
struct Lines<'source> {
    source: &'source str,
    /// Each line's bytes, its line break left out.
    ranges: Vec<Range<usize>>,
}

impl<'source> Lines<'source> {
    fn new(source: &'source str, version: LuaVersion) -> Self {
        let mut ranges = Vec::new();
        let mut line_start = 0;
        for line_break in line_breaks(source.as_bytes(), version) {
            ranges.push(line_start..line_break.start);
            line_start = line_break.end;
        }
        ranges.push(line_start..source.len());
        Self { source, ranges }
    }

    fn text(&self, line: usize) -> &'source str {
        &self.source[self.ranges[line].clone()]
    }

    /// The zero-based line `offset` is on and its byte column, kept within
    /// the line's text.
    fn position(&self, offset: usize) -> (usize, usize) {
        let line = self.ranges.partition_point(|range| range.start <= offset) - 1;
        let range = &self.ranges[line];
        (line, offset.min(range.end) - range.start)
    }

    fn start(&self, span: Span) -> (usize, usize) {
        self.position(Range::<usize>::from(span).start)
    }

    /// Where `span` ends: at the end of the line before when it takes a line
    /// break and nothing after it.
    fn end(&self, span: Span) -> (usize, usize) {
        let Range { start, end } = Range::<usize>::from(span);
        let (line, column) = self.position(end);
        if end > start && column == 0 && line > self.start(span).0 {
            let previous_line = line - 1;
            return (previous_line, self.text(previous_line).len());
        }
        (line, column)
    }
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

/// A message with no fix, at `span`.
pub(crate) fn message(span: Span) -> Message {
    Message { diagnostic: Diagnostic::lint("test", Severity::Warning, span, "test"), fix: None }
}
