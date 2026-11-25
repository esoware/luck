use luck_diagnostics::Tag;
use luck_token::{
    LintExtent, LintVerb, LuaVersion, Span, index_to_offset, is_lua_whitespace, strip_line_break,
};

use crate::context::ContextHost;
use crate::diagnostic::Finding;
use crate::directives::{Directive, Directives, Target};
use crate::fix::TextEdit;
use crate::rule::{Rule, declare_lint_rule};
use crate::rules::RuleId;

/// Flags an `allow` that drops no diagnostic of a rule that ran, with a fix
/// that removes it. One whose rule didn't run may still be needed, so it's
/// left alone.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnusedLintFilter;

declare_lint_rule!(
    UnusedLintFilter,
    Correctness,
    Warning,
    "`allow` directive that silences nothing",
    fix = Fix
);

/// The linter runs it apart from the others, through [`UnusedLintFilter::check`].
impl Rule for UnusedLintFilter {}

impl UnusedLintFilter {
    /// `used` marks each entry whose `allow` dropped a diagnostic, and `runs`
    /// says whether a rule ran.
    ///
    /// What an `allow` of this rule drops is this rule's own reports, which
    /// `used` doesn't count, so one is never reported.
    pub(crate) fn check<Runs>(context: &ContextHost<'_>, used: &[bool], runs: Runs) -> Vec<Finding>
    where
        Runs: Fn(RuleId) -> bool,
    {
        let directives = context.directives;
        let runs_other = |rule: RuleId| rule != RuleId::UnusedLintFilter && runs(rule);
        let has_run_other = RuleId::ALL.into_iter().any(runs_other);
        let is_unused = |entry: usize| {
            !used[entry]
                && match directives.entries()[entry].target {
                    Target::Rule(rule) => runs_other(rule),
                    Target::Every => has_run_other,
                    Target::Unknown => false,
                }
        };
        let mut findings = Vec::new();
        for directive in directives.directives() {
            if directive.verb != LintVerb::Allow || directive.extent == LintExtent::RegionEnd {
                continue;
            }
            let unused: Vec<usize> =
                directive.entries.clone().filter(|&entry| is_unused(entry)).collect();
            if unused.is_empty() {
                continue;
            }
            if unused.len() == directive.entries.len() {
                findings.push(
                    Finding::new("unused `allow` directive", directive.span)
                        .with_tag(Tag::Unnecessary)
                        .with_help("no rule it names reports anything where it applies")
                        .with_directive_fix(context, |fixer| {
                            fixer.fix("remove the unused directive", removal(context, &unused))
                        }),
                );
                continue;
            }
            for entry in unused {
                let found = &directives.entries()[entry];
                let name = String::from_utf8_lossy(found.name.text(context.source));
                let help = if found.target == Target::Every {
                    "no rule reports anything where it applies"
                } else {
                    "the rule reports nothing where it applies"
                };
                findings.push(
                    Finding::new(format!("unused `{name}` in this `allow`"), found.name)
                        .with_tag(Tag::Unnecessary)
                        .with_help(help)
                        .with_directive_fix(context, |fixer| {
                            fixer.fix(
                                format!("remove `{name}` from the directive"),
                                removal(context, &[entry]),
                            )
                        }),
                );
            }
        }
        findings
    }
}

/// The edits that take `removed`, entries of one directive, out of it, and
/// the `end`s that close their regions out of theirs. A directive left with
/// no entry goes whole.
fn removal(context: &ContextHost<'_>, removed: &[usize]) -> Vec<TextEdit> {
    let directives = context.directives;
    let mut entries: Vec<usize> = removed
        .iter()
        .copied()
        .chain(removed.iter().filter_map(|&entry| directives.entries()[entry].end))
        .collect();
    entries.sort_unstable();
    let mut edits = Vec::new();
    let mut remaining = entries.as_slice();
    while let Some(&first) = remaining.first() {
        let directive = &directives.directives()[directives.entries()[first].directive];
        let count = remaining.partition_point(|&entry| entry < directive.entries.end);
        let (taken, rest) = remaining.split_at(count);
        remaining = rest;
        edits.push(if taken.len() == directive.entries.len() {
            comment_removal(context.source, directive.span, context.semantic.version())
        } else {
            list_without(context.source, directives, directive, taken)
        });
    }
    edits
}

/// `directive`'s rule list without `taken`, as one edit, so removing two of
/// its names at once never makes two edits that overlap.
fn list_without(
    source: &[u8],
    directives: &Directives,
    directive: &Directive,
    taken: &[usize],
) -> TextEdit {
    let kept: Vec<&[u8]> = directive
        .entries
        .clone()
        .filter(|entry| !taken.contains(entry))
        .map(|entry| directives.entries()[entry].name.text(source))
        .collect();
    TextEdit { span: directive.list, replacement: kept.join(&b", "[..]).into() }
}

/// The edit that removes the comment at `span`.
///
/// It takes the comment's whole line when nothing else is on it, the blanks
/// before it when it ends a line of code, and else the blanks after it,
/// leaving a space between code on both sides.
fn comment_removal(source: &[u8], span: Span, version: LuaVersion) -> TextEdit {
    let (start, end) = (span.start as usize, span.end as usize);
    let before = &source[..start];
    let after = &source[end..];
    let is_line_break = |b: u8| b == b'\n' || (b == b'\r' && version.has_lfcr_line_break());
    let line_before_length = before.iter().rev().take_while(|&&b| !is_line_break(b)).count();
    let line_after_length = after.iter().take_while(|&&b| !is_line_break(b)).count();
    let line_before = &before[start - line_before_length..];
    let line_after = &after[..line_after_length];
    let has_code_before = !line_before.iter().all(|&b| is_lua_whitespace(b));
    let has_code_after = !line_after.iter().all(|&b| is_lua_whitespace(b));
    let (removed_start, removed_end, replacement) = match (has_code_before, has_code_after) {
        (false, false) => {
            let rest = &after[line_after_length..];
            let line_break_length = strip_line_break(rest, version)
                .map_or(0, |after_break| rest.len() - after_break.len());
            let line_end = end + line_after_length + line_break_length;
            (start - line_before_length, line_end, &b""[..])
        }
        (true, false) => {
            let blanks_before =
                line_before.iter().rev().take_while(|&&b| is_lua_whitespace(b)).count();
            (start - blanks_before, end, &b""[..])
        }
        (_, true) => {
            let blanks_after = after.iter().take_while(|&&b| is_lua_whitespace(b)).count();
            let is_touching = line_before.last().is_some_and(|&b| !is_lua_whitespace(b));
            (start, end + blanks_after, if is_touching { &b" "[..] } else { b"" })
        }
    };
    TextEdit {
        span: Span::new(index_to_offset(removed_start), index_to_offset(removed_end)),
        replacement: replacement.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
-- luck: allow(setting_global)
x = 1
-- luck: allow(*)
y = 1",
        "\
-- luck: allow(shadowing)
local a = 1
print(a)",
        "\
-- luck: deny(undefined_variable)
-- luck: warn(setting_global)
-- luck: allow(not_a_rule)
local a = 1
print(a)",
        "\
-- #luck: allow(unused_lint_filter)
-- luck: allow(undefined_variable)
local a = 1
print(a)",
    ];

    const FAIL: &[&str] = &[
        "\
local a = 1
-- luck: allow(undefined_variable)
print(a)",
        "\
-- luck: allow(undefined_variable, setting_global)
x = 1",
        "\
local a = 1 -- luck: allow(undefined_variable)
print(a)",
        "print(1)--[[ luck: allow(undefined_variable) ]]print(2)",
        "print(1) --[[ luck: allow(undefined_variable) ]] print(2)",
        "\
-- luck: allow(undefined_variable) start
local a = 1
-- luck: allow(undefined_variable, setting_global) end
print(a)",
        "\
-- luck: allow(*)
local a = 1
print(a)",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnusedLintFilter, PASS, FAIL).test_and_snapshot();
    }
}
