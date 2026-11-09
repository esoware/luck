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

/// Applies the diagnostics' fixes to `source` in one pass.
///
/// Overlap is checked per edit, not per fix: of two overlapping edits, the
/// one starting later wins. If the result doesn't parse, the original source
/// comes back unchanged, so a broken fix never reaches the user's file.
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
    let mut edits: Vec<&TextEdit> =
        diags.iter().filter_map(|diag| diag.fix.as_ref()).flat_map(|fix| &fix.edits).collect();

    if edits.is_empty() {
        return source.to_owned();
    }

    // Sorting on the full key puts edits at the same offset, such as two
    // inserts, in a fixed order, whichever rule produced them.
    edits.sort_by(|left, right| {
        right
            .span
            .start
            .cmp(&left.span.start)
            .then(right.span.end.cmp(&left.span.end))
            .then(left.replacement.cmp(&right.replacement))
    });

    let mut fixed = source.to_owned();
    let mut last_start = u32::MAX;

    for edit in &edits {
        if edit.span.end <= last_start {
            fixed.replace_range(Range::<usize>::from(edit.span), &edit.replacement);
            last_start = edit.span.start;
        }
    }

    fixed
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
