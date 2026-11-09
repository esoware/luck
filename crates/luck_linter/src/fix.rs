use alloc::collections::BTreeMap;
use core::cmp::Reverse;
use core::error::Error;
use core::fmt;
use core::ops::Range;

use luck_core::LintConfig;
use luck_token::{Comment, LuaVersion, Span};

use crate::diagnostic::LintDiagnostic;
use crate::driver::lint;

/// Maximum fix-and-relint rounds. Needing more almost always means two rules
/// are undoing each other.
pub const FIXPOINT_BUDGET: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub description: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub span: Span,
    pub replacement: String,
}

/// The comments that sit inside `span`, which a fix rewriting `span` would
/// delete. `comments` is in source order.
pub(crate) fn comments_within(comments: &[Comment], span: Span) -> &[Comment] {
    let first = comments.partition_point(|comment| comment.span.start < span.start);
    let last = comments.partition_point(|comment| comment.span.end <= span.end);
    comments.get(first..last).unwrap_or_default()
}

/// Applies the diagnostics' [`non_overlapping_edits`] to `source` in one pass.
///
/// If the result doesn't parse, the original source comes back unchanged, so a
/// broken fix never reaches the user's file.
///
/// [`apply_fixes_fixpoint`] re-lints and repeats until no fixes remain.
#[must_use]
pub fn apply_fixes(source: &str, diags: &[LintDiagnostic], version: LuaVersion) -> String {
    let fixed = apply_one_pass(source, diags);
    if fixed != source && !luck_parser::parse(&fixed, version).errors.is_empty() {
        return source.to_owned();
    }
    fixed
}

pub(crate) fn apply_one_pass(source: &str, diags: &[LintDiagnostic]) -> String {
    let mut fixed = String::with_capacity(source.len());
    let mut copied_to = 0;
    for edit in non_overlapping_edits(diags) {
        let Range { start, end } = Range::<usize>::from(edit.span);
        fixed.push_str(source.get(copied_to..start).unwrap_or_default());
        fixed.push_str(&edit.replacement);
        copied_to = end;
    }
    fixed.push_str(source.get(copied_to..).unwrap_or_default());
    fixed
}

/// The edits of every fix that overlaps none taken so far, in source order.
///
/// The fixes reaching furthest into the source are taken first, so of two
/// overlapping ones the later wins. A fix is kept or dropped whole, because
/// half of a multi-edit fix changes the code's meaning, and a dropped one
/// comes back on the next pass. Edits at one offset keep a fixed order,
/// whichever rule produced them.
#[must_use]
pub fn non_overlapping_edits(diags: &[LintDiagnostic]) -> Vec<&TextEdit> {
    let mut fixes: Vec<&Fix> = diags.iter().filter_map(|diag| diag.fix.as_ref()).collect();
    fixes.sort_by_key(|fix| {
        let furthest_edit = fix
            .edits
            .iter()
            .map(|edit| (edit.span.start, edit.span.end, Reverse(&edit.replacement)))
            .max();
        Reverse(furthest_edit)
    });
    // Each kept span's end by its start, so a candidate has two neighbors to
    // check. Two inserts at one offset don't conflict, and the wider span at
    // an offset is the one that can.
    let mut kept_spans: BTreeMap<u32, u32> = BTreeMap::new();
    let mut edits: Vec<&TextEdit> = Vec::new();
    for fix in fixes {
        if fix.edits.iter().any(|edit| overlaps_kept(&kept_spans, edit.span)) {
            continue;
        }
        for edit in &fix.edits {
            let end = kept_spans.entry(edit.span.start).or_insert(edit.span.end);
            *end = edit.span.end.max(*end);
        }
        edits.extend(&fix.edits);
    }
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end, Reverse(&edit.replacement)));
    edits
}

/// Half-open overlap, so edits that only touch, like an insert at the end of
/// a replaced range, don't conflict.
fn overlaps_kept(kept_spans: &BTreeMap<u32, u32>, span: Span) -> bool {
    let is_overlapping = |(&start, &end): (&u32, &u32)| start < span.end && span.start < end;
    kept_spans.range(..=span.start).next_back().is_some_and(is_overlapping)
        || kept_spans.range(span.start..).next().is_some_and(is_overlapping)
}

/// Why [`apply_fixes_fixpoint`] stopped without converging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixpointError {
    /// The rounds ran out while rules were still producing fixes.
    BudgetExhausted { iterations: usize },
    /// A round's output didn't parse. `iteration` counts from 0.
    ReparseFailed { iteration: usize, message: String },
}

impl fmt::Display for FixpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetExhausted { iterations } => write!(
                f,
                "lint auto-fix did not converge within {iterations} iterations (possible rule cycle)"
            ),
            Self::ReparseFailed { iteration, message } => write!(
                f,
                "lint auto-fix produced unparseable output on iteration {iteration}: {message}"
            ),
        }
    }
}

impl Error for FixpointError {}

/// Lints `source` with [`crate::lint`] and applies the fixes, repeating until
/// no fixes remain.
///
/// # Errors
///
/// [`FixpointError::BudgetExhausted`] when fixes are still coming after
/// [`FIXPOINT_BUDGET`] rounds, and [`FixpointError::ReparseFailed`] when a
/// round's output doesn't parse.
pub fn apply_fixes_fixpoint(
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
) -> Result<String, FixpointError> {
    fixpoint_within(source, version, config, FIXPOINT_BUDGET)
}

/// [`apply_fixes_fixpoint`] with a caller-chosen round limit.
///
/// # Errors
///
/// As for [`apply_fixes_fixpoint`], with `budget` as the limit.
fn fixpoint_within(
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
    budget: usize,
) -> Result<String, FixpointError> {
    let mut current = source.to_owned();
    for iteration in 0..budget {
        let diags = lint(&current, version, config);
        let has_fix = diags.iter().any(|diag| diag.fix.is_some());
        if !has_fix {
            return Ok(current);
        }
        let next = apply_one_pass(&current, &diags);
        if next == current {
            // The fixes changed nothing, so this is a fixed point.
            return Ok(current);
        }
        let parse = luck_parser::parse(&next, version);
        if !parse.errors.is_empty() {
            return Err(FixpointError::ReparseFailed {
                iteration,
                message: parse
                    .errors
                    .iter()
                    .map(|error| error.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        current = next;
    }
    Err(FixpointError::BudgetExhausted { iterations: budget })
}

#[cfg(test)]
mod tests {
    use luck_core::RuleSetting;

    use super::*;
    use crate::fixtures::diagnostic;

    fn diag_with_edit(start: u32, end: u32, replacement: &str) -> LintDiagnostic {
        let span = Span { start, end };
        LintDiagnostic {
            fix: Some(Fix {
                description: "test fix".to_owned(),
                edits: vec![TextEdit { span, replacement: replacement.to_owned() }],
            }),
            ..diagnostic("test", span)
        }
    }

    /// The default config with `name` turned on.
    fn enable(name: &str) -> LintConfig {
        let mut config = LintConfig::default();
        config
            .rule_overrides
            .insert(name.to_owned(), RuleSetting { enabled: Some(true), severity: None });
        config
    }

    #[test]
    fn applies_one_fix() {
        let source = "local foo = 1";
        let diags = vec![diag_with_edit(6, 9, "_foo")];
        let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local _foo = 1");
    }

    #[test]
    fn applies_every_non_overlapping_fix() {
        let source = "local foo = 1\nlocal bar = 2";
        let diags = vec![diag_with_edit(6, 9, "_foo"), diag_with_edit(20, 23, "_bar")];
        let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local _foo = 1\nlocal _bar = 2");
    }

    #[test]
    fn overlapping_edit_that_starts_later_wins() {
        let source = "abcdefghij";
        let diags = vec![diag_with_edit(2, 6, "XX"), diag_with_edit(4, 8, "YY")];
        // Mechanics test on non-Lua text, so bypass the reparse guard.
        let fixed = apply_one_pass(source, &diags);
        assert_eq!(fixed, "abcdYYij");
    }

    #[test]
    fn a_fix_overlapped_by_a_later_one_drops_whole() {
        let source = "abcdefghij";
        let span = Span { start: 0, end: 1 };
        let overlapped_span = Span { start: 2, end: 6 };
        let two_edit_fix = LintDiagnostic {
            fix: Some(Fix {
                description: "two edits".to_owned(),
                edits: vec![
                    TextEdit { span, replacement: "X".to_owned() },
                    TextEdit { span: overlapped_span, replacement: "Y".to_owned() },
                ],
            }),
            ..diagnostic("test", span)
        };
        let diags = vec![two_edit_fix, diag_with_edit(5, 8, "Z")];
        // Mechanics test on non-Lua text, so bypass the reparse guard.
        assert_eq!(apply_one_pass(source, &diags), "abcdeZij");
    }

    #[test]
    fn unparseable_result_returns_source_unchanged() {
        let source = "local foo = 1";
        let diags = vec![diag_with_edit(6, 9, "end")];
        let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, source);
    }

    #[test]
    fn diags_without_fixes_leave_source_unchanged() {
        let source = "local x = 1";
        let diags = vec![diagnostic("test", Span { start: 6, end: 7 })];
        let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, source);
    }

    #[test]
    fn no_diags_leave_source_unchanged() {
        let source = "local x = 1";
        let fixed = apply_fixes(source, &[], LuaVersion::Lua54);
        assert_eq!(fixed, source);
    }

    #[test]
    fn fixpoint_applies_fix_from_one_rule() {
        // unused_variable will rename `unused` -> `_unused`; once renamed
        // it's no longer flagged.
        let config = enable("unused_variable");
        let source = "local unused = 1";
        let fixed = apply_fixes_fixpoint(source, LuaVersion::Lua54, &config).expect("fixpoint");
        assert_eq!(fixed, "local _unused = 1");
    }

    #[test]
    fn fixpoint_applies_fixes_from_two_rules() {
        // `redundant_nil_init` drops the `= nil` and `unused_variable`
        // prefixes the name. The edits don't overlap, so both land in the
        // first round and the second finds nothing left to fix.
        let config = enable("redundant_nil_init");
        let source = "local unused = nil";
        let fixed = apply_fixes_fixpoint(source, LuaVersion::Lua54, &config).expect("fixpoint");
        assert_eq!(fixed, "local _unused");
    }

    #[test]
    fn fixpoint_returns_source_when_no_rule_fires() {
        let config = LintConfig::default();
        let source = "local x = 1\nreturn x";
        let result = apply_fixes_fixpoint(source, LuaVersion::Lua54, &config);
        assert_eq!(result.expect("fixpoint"), source);
    }

    #[test]
    fn fixpoint_reports_budget_exhausted() {
        // The first round applies both fixes, and it takes a second round
        // to see that none remain.
        let config = enable("redundant_nil_init");
        let result = fixpoint_within("local unused = nil", LuaVersion::Lua54, &config, 1);
        assert_eq!(
            result,
            Err(FixpointError::BudgetExhausted { iterations: 1 }),
            "one round can't confirm that no fixes remain"
        );
    }

    #[test]
    fn fixpoint_converges_within_its_budget() {
        let config = enable("redundant_nil_init");
        let result = fixpoint_within("local unused = nil", LuaVersion::Lua54, &config, 2);
        assert_eq!(result.expect("fixpoint"), "local _unused");
    }
}
