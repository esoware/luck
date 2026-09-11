use crate::LintConfig;
use crate::diagnostic::{LintDiagnostic, TextEdit};
use luck_token::LuaVersion;

/// Maximum fixpoint iterations before giving up. Needing more passes
/// than this means two rules are cycling, so the caller should
/// investigate rather than spin.
pub const FIXPOINT_BUDGET: usize = 10;

/// Apply fixes from diagnostics to source text in a single pass.
///
/// Returns the modified source text. Only applies fixes that don't
/// overlap; when fixes conflict, the one with the higher start byte
/// wins (descending-sort iteration drops any later overlap).
///
/// Hard invariant 8. If the edited result no longer parses, this
/// returns the original source untouched, because a broken fix must
/// never reach the user's file.
///
/// For multi-pass / re-lint behavior, prefer `apply_fixes_fixpoint`.
pub fn apply_fixes(source: &str, diagnostics: &[LintDiagnostic], version: LuaVersion) -> String {
    let result = apply_one_pass(source, diagnostics);
    if result != source && !luck_parser::parse(&result, version).errors.is_empty() {
        return source.to_string();
    }
    result
}

fn apply_one_pass(source: &str, diagnostics: &[LintDiagnostic]) -> String {
    let mut edits: Vec<&TextEdit> = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.fix.as_ref())
        .flat_map(|fix| &fix.edits)
        .collect();

    if edits.is_empty() {
        return source.to_string();
    }

    // Full-key stable sort: same-offset edits (two inserts at one
    // position) apply in one deterministic order regardless of which
    // rule produced them first.
    edits.sort_by(|a, b| {
        b.span
            .start
            .cmp(&a.span.start)
            .then(b.span.end.cmp(&a.span.end))
            .then(a.replacement.cmp(&b.replacement))
    });

    let mut result = source.to_string();
    let mut last_start = u32::MAX;

    for edit in &edits {
        if edit.span.end <= last_start {
            let start = edit.span.start as usize;
            let end = edit.span.end as usize;
            result.replace_range(start..end, &edit.replacement);
            last_start = edit.span.start;
        }
    }

    result
}

/// Reason a fixpoint iteration ended without converging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixpointError {
    /// The loop spent its whole budget and rules were still producing
    /// fixes, most likely two rules undoing each other.
    BudgetExhausted { iterations: usize },
    /// Output of one round failed to re-parse. Carries the round
    /// number where parsing broke.
    ReparseFailed { iteration: usize, message: String },
}

impl std::fmt::Display for FixpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FixpointError::BudgetExhausted { iterations } => write!(
                f,
                "lint auto-fix did not converge within {iterations} iterations (possible rule cycle)"
            ),
            FixpointError::ReparseFailed { iteration, message } => write!(
                f,
                "lint auto-fix produced unparseable output on iteration {iteration}: {message}"
            ),
        }
    }
}

impl std::error::Error for FixpointError {}

/// Re-run lint and apply fixes until no more fixes are produced or the
/// iteration budget is exhausted.
pub fn apply_fixes_fixpoint(
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
) -> Result<String, FixpointError> {
    fixpoint_within(source, version, config, FIXPOINT_BUDGET)
}

fn fixpoint_within(
    source: &str,
    version: LuaVersion,
    config: &LintConfig,
    budget: usize,
) -> Result<String, FixpointError> {
    let mut current = source.to_string();
    for iteration in 0..budget {
        let diagnostics = crate::lint(&current, version, config);
        let has_fix = diagnostics.iter().any(|d| d.fix.is_some());
        if !has_fix {
            return Ok(current);
        }
        let next = apply_one_pass(&current, &diagnostics);
        if next == current {
            // Edits all collided with each other and produced no
            // change, so this is a fixed point.
            return Ok(current);
        }
        // Catch a rule that produces ungrammatical output before the
        // next iteration wastes work on it.
        let parse = luck_parser::parse(&next, version);
        if !parse.errors.is_empty() {
            return Err(FixpointError::ReparseFailed {
                iteration,
                message: parse
                    .errors
                    .iter()
                    .map(|e| e.message.clone())
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
    use super::*;
    use crate::diagnostic::*;
    use luck_token::Span;

    fn make_diagnostic(start: u32, end: u32, replacement: &str) -> LintDiagnostic {
        LintDiagnostic {
            rule: "test",
            category: Category::Correctness,
            severity: Severity::Warning,
            message: "test".to_string(),
            span: Span { start, end },
            help: None,
            fix: Some(Fix {
                description: "test fix".to_string(),
                edits: vec![TextEdit {
                    span: Span { start, end },
                    replacement: replacement.to_string(),
                }],
            }),
        }
    }

    #[test]
    fn single_fix() {
        let source = "local foo = 1";
        let diagnostics = vec![make_diagnostic(6, 9, "_foo")];
        let result = apply_fixes(source, &diagnostics, LuaVersion::Lua54);
        assert_eq!(result, "local _foo = 1");
    }

    #[test]
    fn multiple_non_overlapping_fixes() {
        let source = "local foo = 1\nlocal bar = 2";
        let diagnostics = vec![
            make_diagnostic(6, 9, "_foo"),
            make_diagnostic(20, 23, "_bar"),
        ];
        let result = apply_fixes(source, &diagnostics, LuaVersion::Lua54);
        assert_eq!(result, "local _foo = 1\nlocal _bar = 2");
    }

    #[test]
    fn overlapping_fixes_first_wins() {
        let source = "abcdefghij";
        let diagnostics = vec![make_diagnostic(2, 6, "XX"), make_diagnostic(4, 8, "YY")];
        // Mechanics test on non-Lua text, so bypass the reparse guard.
        let result = apply_one_pass(source, &diagnostics);
        assert_eq!(result, "abcdYYij");
    }

    #[test]
    fn no_fixes_returns_unchanged() {
        let source = "local x = 1";
        let diagnostics = vec![LintDiagnostic {
            rule: "test",
            category: Category::Correctness,
            severity: Severity::Warning,
            message: "test".to_string(),
            span: Span { start: 6, end: 7 },
            help: None,
            fix: None,
        }];
        let result = apply_fixes(source, &diagnostics, LuaVersion::Lua54);
        assert_eq!(result, source);
    }

    #[test]
    fn empty_diagnostics() {
        let source = "local x = 1";
        let result = apply_fixes(source, &[], LuaVersion::Lua54);
        assert_eq!(result, source);
    }

    #[test]
    fn fixpoint_converges_in_one_pass() {
        // unused_variable will rename `unused` -> `_unused`; once renamed
        // it's no longer flagged.
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            "unused_variable".to_string(),
            crate::RuleSetting {
                enabled: Some(true),
                severity: None,
            },
        );
        let source = "local unused = 1";
        let result = apply_fixes_fixpoint(source, LuaVersion::Lua54, &config).expect("fixpoint");
        assert_eq!(result, "local _unused = 1");
    }

    #[test]
    fn fixpoint_chains_two_rules() {
        // redundant_nil_init drops the `= nil`, producing `local unused`,
        // which `unused_variable` then catches and prefixes with `_`.
        // Two iterations, then stable.
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            "redundant_nil_init".to_string(),
            crate::RuleSetting {
                enabled: Some(true),
                severity: None,
            },
        );
        let source = "local unused = nil";
        let result = apply_fixes_fixpoint(source, LuaVersion::Lua54, &config).expect("fixpoint");
        assert_eq!(result, "local _unused");
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
        // `local unused = nil` needs two rounds: `redundant_nil_init`
        // drops the initializer, then `unused_variable` prefixes the
        // name. One round leaves a fix still pending.
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            "redundant_nil_init".to_string(),
            crate::RuleSetting {
                enabled: Some(true),
                severity: None,
            },
        );
        let result = fixpoint_within("local unused = nil", LuaVersion::Lua54, &config, 1);
        assert_eq!(
            result,
            Err(FixpointError::BudgetExhausted { iterations: 1 }),
            "one round should leave a pending fix"
        );
    }

    #[test]
    fn fixpoint_converges_within_its_budget() {
        let mut config = LintConfig::default();
        config.rule_overrides.insert(
            "redundant_nil_init".to_string(),
            crate::RuleSetting {
                enabled: Some(true),
                severity: None,
            },
        );
        let result = fixpoint_within("local unused = nil", LuaVersion::Lua54, &config, 2);
        assert_eq!(result.expect("fixpoint"), "local _unused");
    }
}
