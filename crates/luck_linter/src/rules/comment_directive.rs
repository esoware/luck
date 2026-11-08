use core::mem;

use luck_ast::Block;
use luck_token::{CommentKind, Span};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule, WholeRule};
use crate::suggest::levenshtein;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommentDirective;

const KNOWN_DIRECTIVES: &[&str] =
    &["nolint", "nocheck", "nonstrict", "strict", "optimize", "native"];

impl Rule for CommentDirective {
    fn name(&self) -> &'static str {
        "comment_directive"
    }

    fn category(&self) -> Category {
        Category::Correctness
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "invalid or misplaced `--!` comment directive"
    }
}

impl WholeRule for CommentDirective {
    fn check(&self, context: &LintContext<'_>) -> Vec<LintDiagnostic> {
        if !context.semantic.version.is_luau() {
            return Vec::new();
        }
        let first_code_start = first_code_start(context.block);
        let mut has_seen_mode = false;
        let mut diags = Vec::new();
        for comment in context.comments {
            if comment.kind != CommentKind::Line {
                continue;
            }
            let Some(body) = comment.span.text(context.source).strip_prefix("--!") else {
                continue;
            };
            // Luau ignores hot comments after the first statement.
            let problem = if first_code_start.is_some_and(|start| comment.span.start > start) {
                Some(LintDiagnostic::new(
                    "comment_directive",
                    "comment directive after the first statement has no effect",
                    comment.span,
                ))
            } else {
                directive_problem(body, comment.span.start.saturating_add(3), &mut has_seen_mode)
            };
            diags.extend(problem);
        }
        diags
    }
}

fn first_code_start(block: &Block) -> Option<u32> {
    let stmt_start = block.stmts.first().map(|stmt| stmt.span().start);
    let last_start = block.last_stmt.as_ref().map(|last| last.span().start);
    stmt_start.into_iter().chain(last_start).min()
}

/// Checks the directive `body` that follows `--!`, starting at `word_start`
/// in source. A valid type checking mode directive sets `has_seen_mode`.
fn directive_problem(
    body: &str,
    word_start: u32,
    has_seen_mode: &mut bool,
) -> Option<LintDiagnostic> {
    let word_length = body.find(char::is_whitespace).unwrap_or(body.len());
    let (word, rest) = body.split_at(word_length);
    let word_span = Span::from_indices(0, word_length).shifted(word_start);
    let (message, span) = match word {
        // `nolint` arguments name Luau's lints, not luck's.
        "nolint" => return None,
        "native" => (
            "`native` directive has extra symbols at the end of the line".to_owned(),
            trailing_symbols_span(rest, word_span.end)?,
        ),
        "nocheck" | "nonstrict" | "strict" => {
            if let Some(extra_span) = trailing_symbols_span(rest, word_span.end) {
                (
                    "type checking mode directive has extra symbols at the end of the line"
                        .to_owned(),
                    extra_span,
                )
            } else if mem::replace(has_seen_mode, true) {
                ("type checking mode is already set by an earlier directive".to_owned(), word_span)
            } else {
                return None;
            }
        }
        "optimize" => {
            let Some(level_span) = trailing_symbols_span(rest, word_span.end) else {
                return Some(LintDiagnostic::new(
                    "comment_directive",
                    "`optimize` directive requires an optimization level",
                    word_span,
                ));
            };
            let level = rest.trim();
            if matches!(level, "0" | "1" | "2") {
                return None;
            }
            (format!("unknown optimization level `{level}`, expected `0`, `1`, or `2`"), level_span)
        }
        unknown => {
            let suggestion_hint = closest_directive(unknown)
                .map_or_else(String::new, |suggestion| format!(", did you mean `{suggestion}`?"));
            (format!("unknown comment directive `{unknown}`{suggestion_hint}"), word_span)
        }
    };
    Some(LintDiagnostic::new("comment_directive", message, span))
}

/// Returns the span of whatever follows a directive word, or `None` if only
/// whitespace does. `base` is the offset where `rest` starts.
fn trailing_symbols_span(rest: &str, base: u32) -> Option<Span> {
    let trimmed = rest.trim_end();
    let first = trimmed.find(|character: char| !character.is_whitespace())?;
    Some(Span::from_indices(first, trimmed.len()).shifted(base))
}

fn closest_directive(word: &str) -> Option<&'static str> {
    KNOWN_DIRECTIVES
        .iter()
        .copied()
        .map(|candidate| (levenshtein(word, candidate), candidate))
        .filter(|&(distance, _)| distance <= 2)
        .min_by_key(|&(distance, _)| distance)
        .map(|(_, candidate)| candidate)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&CommentDirective, source, LuaVersion::Luau)
    }

    #[test]
    fn flags_unknown_directive() {
        let diags = run("--!foobar\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("unknown comment directive `foobar`"));
        assert!(!diags[0].message.contains("did you mean"));
    }

    #[test]
    fn flags_unknown_directive_with_suggestion() {
        let diags = run("--!strcit\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("did you mean `strict`"), "{diags:?}");
    }

    #[test]
    fn flags_directive_after_first_statement() {
        let diags = run("local _x = 1\n--!strict");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("after the first statement"));
    }

    #[test]
    fn flags_directive_after_lone_return() {
        let diags = run("return 1\n--!strict");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("after the first statement"));
    }

    #[test]
    fn flags_extra_symbols_after_mode() {
        let diags = run("--!strict yes\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("extra symbols"));
    }

    #[test]
    fn flags_second_mode_directive() {
        let diags = run("--!strict\n--!nocheck\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("set by an earlier directive"));
    }

    #[test]
    fn flags_optimize_without_level() {
        let diags = run("--!optimize\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("requires an optimization level"));
    }

    #[test]
    fn flags_optimize_bad_level() {
        let diags = run("--!optimize 3\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].message.contains("unknown optimization level `3`, expected `0`, `1`, or `2`"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_extra_symbols_after_native() {
        let diags = run("--!native on\nlocal _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("extra symbols"));
    }

    #[test]
    fn ignores_valid_directives() {
        let diags = run("--!strict\n--!native\n--!optimize 2\nlocal _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_nolint_with_argument() {
        let diags = run("--!nolint UnknownGlobal\nlocal _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_plain_comments() {
        let diags = run("-- not a directive\nlocal _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_luau() {
        let diags = run_rule(&CommentDirective, "--!foobar\nlocal _x = 1", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
