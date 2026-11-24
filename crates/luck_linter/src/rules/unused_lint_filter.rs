use luck_diagnostics::Tag;
use luck_token::{
    LintExtent, LintVerb, LuaVersion, Span, index_to_offset, is_lua_whitespace, offset_to_index,
    strip_line_break,
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
            !used.get(entry).copied().unwrap_or(true)
                && directives.entries().get(entry).is_some_and(|found| match found.target {
                    Target::Rule(rule) => runs_other(rule),
                    Target::Every => has_run_other,
                    Target::Unknown => false,
                })
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
                let Some(found) = directives.entries().get(entry) else {
                    continue;
                };
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
        .chain(removed.iter().filter_map(|&entry| directives.entries().get(entry)?.end))
        .collect();
    entries.sort_unstable();
    let mut edits = Vec::new();
    let mut remaining = entries.as_slice();
    while let Some(&first) = remaining.first() {
        let Some(directive) = directives
            .entries()
            .get(first)
            .and_then(|entry| directives.directives().get(entry.directive))
        else {
            break;
        };
        let count = remaining.partition_point(|&entry| entry < directive.entries.end);
        let (taken, rest) = remaining.split_at_checked(count).unwrap_or((remaining, &[]));
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
        .filter_map(|entry| Some(directives.entries().get(entry)?.name.text(source)))
        .collect();
    TextEdit { span: directive.list, replacement: kept.join(&b", "[..]).into() }
}

/// The edit that removes the comment at `span`.
///
/// It takes the comment's whole line when nothing else is on it, the blanks
/// before it when it ends a line of code, and else the blanks after it,
/// leaving a space between code on both sides.
fn comment_removal(source: &[u8], span: Span, version: LuaVersion) -> TextEdit {
    let (start, end) = (offset_to_index(span.start), offset_to_index(span.end));
    let before = source.get(..start).unwrap_or_default();
    let after = source.get(end..).unwrap_or_default();
    let is_line_break = |b: u8| b == b'\n' || (b == b'\r' && version.has_lfcr_line_break());
    let line_before_length = before.iter().rev().take_while(|&&b| !is_line_break(b)).count();
    let line_after_length = after.iter().take_while(|&&b| !is_line_break(b)).count();
    let line_before = before.get(start.saturating_sub(line_before_length)..).unwrap_or_default();
    let line_after = after.get(..line_after_length).unwrap_or_default();
    let has_code_before = !line_before.iter().all(|&b| is_lua_whitespace(b));
    let has_code_after = !line_after.iter().all(|&b| is_lua_whitespace(b));
    let (removed_start, removed_end, replacement) = match (has_code_before, has_code_after) {
        (false, false) => {
            let rest = after.get(line_after_length..).unwrap_or_default();
            let line_break_length = strip_line_break(rest, version)
                .map_or(0, |after_break| rest.len().saturating_sub(after_break.len()));
            let line_end = end.saturating_add(line_after_length).saturating_add(line_break_length);
            (start.saturating_sub(line_before_length), line_end, &b""[..])
        }
        (true, false) => {
            let blanks_before =
                line_before.iter().rev().take_while(|&&b| is_lua_whitespace(b)).count();
            (start.saturating_sub(blanks_before), end, &b""[..])
        }
        (_, true) => {
            let blanks_after = after.iter().take_while(|&&b| is_lua_whitespace(b)).count();
            let is_touching = line_before.last().is_some_and(|&b| !is_lua_whitespace(b));
            (start, end.saturating_add(blanks_after), if is_touching { &b" "[..] } else { b"" })
        }
    };
    TextEdit {
        span: Span::new(index_to_offset(removed_start), index_to_offset(removed_end)),
        replacement: replacement.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::UnusedLintFilter, source, LuaVersion::Lua54)
    }

    fn fixed(source: &str) -> String {
        fix_and_reparse(source, &run(source), LuaVersion::Lua54)
    }

    #[test]
    fn flags_an_allow_that_silences_nothing_and_removes_its_line() {
        let source = "\
local a = 1
-- luck: allow(undefined_variable)
print(a)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "unused `allow` directive");
        assert_eq!(
            fixed(source),
            "\
local a = 1
print(a)"
        );
    }

    #[test]
    fn flags_one_name_of_several_and_removes_only_it() {
        let source = "\
-- luck: allow(undefined_variable, setting_global)
x = 1";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "unused `undefined_variable` in this `allow`");
        assert_eq!(diags[0].span().text(source.as_bytes()), b"undefined_variable");
        assert_eq!(
            fixed(source),
            "\
-- luck: allow(setting_global)
x = 1"
        );
    }

    #[test]
    fn flags_a_trailing_allow_and_keeps_the_code() {
        let source = "\
local a = 1 -- luck: allow(undefined_variable)
print(a)";
        assert_eq!(
            fixed(source),
            "\
local a = 1
print(a)"
        );
    }

    #[test]
    fn flags_a_block_allow_between_code_and_keeps_it_apart() {
        assert_eq!(
            fixed("print(1)--[[ luck: allow(undefined_variable) ]]print(2)"),
            "print(1) print(2)"
        );
        assert_eq!(
            fixed("print(1) --[[ luck: allow(undefined_variable) ]] print(2)"),
            "print(1) print(2)"
        );
    }

    #[test]
    fn flags_an_unused_region_and_removes_its_end_too() {
        let source = "\
-- luck: allow(undefined_variable) start
local a = 1
-- luck: allow(undefined_variable, setting_global) end
print(a)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            fixed(source),
            "\
local a = 1
-- luck: allow(setting_global) end
print(a)"
        );
    }

    #[test]
    fn flags_an_unused_wildcard() {
        let source = "\
-- luck: allow(*)
local a = 1
print(a)";
        assert_eq!(run(source).len(), 1);
    }

    #[test]
    fn ignores_an_allow_that_silences_something() {
        let diags = run("\
-- luck: allow(setting_global)
x = 1
-- luck: allow(*)
y = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_an_allow_whose_rule_did_not_run() {
        let diags = run("\
-- luck: allow(shadowing)
local a = 1
print(a)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_deny_warn_and_unknown_names() {
        let diags = run("\
-- luck: deny(undefined_variable)
-- luck: warn(setting_global)
-- luck: allow(not_a_rule)
local a = 1
print(a)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_an_allow_of_itself_and_what_one_covers() {
        let diags = run("\
-- #luck: allow(unused_lint_filter)
-- luck: allow(undefined_variable)
local a = 1
print(a)");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
