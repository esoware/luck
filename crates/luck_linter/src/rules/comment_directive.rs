use core::mem;

use luck_ast::Block;
use luck_token::{CommentKind, Span};

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};
use crate::suggest::closest;

const KNOWN_DIRECTIVES: &[&str] =
    &["nolint", "nocheck", "nonstrict", "strict", "optimize", "native"];

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommentDirective;

impl_rule!(
    CommentDirective,
    "comment_directive",
    Correctness,
    Warning,
    "invalid or misplaced `--!` comment directive"
);

impl WholeRule for CommentDirective {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        if !context.semantic.version.is_luau() {
            return Vec::new();
        }
        let first_code_start = first_code_start(context.block);
        let mut has_seen_mode = false;
        let mut findings = Vec::new();
        for comment in context.comments {
            if comment.kind != CommentKind::Line {
                continue;
            }
            let Some(body) = comment.span.text(context.source).strip_prefix("--!") else {
                continue;
            };
            // Luau reserves `--!` and `--! ...` for plain comments.
            if body.is_empty() || body.starts_with([' ', '\t']) {
                continue;
            }
            // Luau ignores hot comments after the first statement.
            if first_code_start.is_some_and(|start| comment.span.start > start) {
                findings.push(Finding::new(
                    "comment directive after the first statement has no effect",
                    comment.span,
                ));
                continue;
            }
            let hot_comment = HotComment::split(body, comment.span.start.saturating_add(3));
            if hot_comment.sets_mode() {
                if mem::replace(&mut has_seen_mode, true) {
                    findings.push(Finding::new(
                        "type checking mode is already set by an earlier directive",
                        hot_comment.word_span,
                    ));
                }
            } else {
                findings.extend(hot_comment.problem());
            }
        }
        findings
    }
}

fn first_code_start(block: &Block) -> Option<u32> {
    let stmt_start = block.stmts.first().map(|stmt| stmt.span().start);
    let last_start = block.last_stmt.as_ref().map(|last| last.span().start);
    stmt_start.into_iter().chain(last_start).min()
}

/// The text after a `--!`, split into its directive word and the rest.
struct HotComment<'body> {
    word: &'body str,
    rest: &'body str,
    word_span: Span,
}

impl<'body> HotComment<'body> {
    /// Splits `body`, which starts at `word_start` in source.
    fn split(body: &'body str, word_start: u32) -> Self {
        let word_length = body.find(char::is_whitespace).unwrap_or(body.len());
        let (word, rest) = body.split_at(word_length);
        Self { word, rest, word_span: Span::from_indices(0, word_length).shifted(word_start) }
    }

    /// The span of whatever follows the word, or `None` if only whitespace
    /// does.
    fn trailing_symbols_span(&self) -> Option<Span> {
        let trimmed = self.rest.trim_end();
        let first = trimmed.find(|character: char| !character.is_whitespace())?;
        Some(Span::from_indices(first, trimmed.len()).shifted(self.word_span.end))
    }

    /// Whether the word names a type checking mode.
    fn is_mode(&self) -> bool {
        matches!(self.word, "nocheck" | "nonstrict" | "strict")
    }

    /// Whether this is a well-formed type checking mode directive.
    fn sets_mode(&self) -> bool {
        self.is_mode() && self.trailing_symbols_span().is_none()
    }

    /// What's wrong with this directive, short of repeating an earlier mode.
    fn problem(&self) -> Option<Finding> {
        let (message, span) = match self.word {
            // `nolint` arguments name Luau's lints, not luck's.
            "nolint" => return None,
            "native" => (
                "`native` directive has extra symbols at the end of the line".to_owned(),
                self.trailing_symbols_span()?,
            ),
            _ if self.is_mode() => (
                "type checking mode directive has extra symbols at the end of the line".to_owned(),
                self.trailing_symbols_span()?,
            ),
            "optimize" => {
                let Some(level_span) = self.trailing_symbols_span() else {
                    return Some(Finding::new(
                        "`optimize` directive requires an optimization level",
                        self.word_span,
                    ));
                };
                let level = self.rest.trim();
                if matches!(level, "0" | "1" | "2") {
                    return None;
                }
                (
                    format!("unknown optimization level `{level}`, expected `0`, `1`, or `2`"),
                    level_span,
                )
            }
            unknown => {
                let suggestion_hint =
                    closest(unknown, KNOWN_DIRECTIVES.iter().copied(), |distance, _| distance <= 2)
                        .map_or_else(String::new, |suggestion| {
                            format!(", did you mean `{suggestion}`?")
                        });
                (format!("unknown comment directive `{unknown}`{suggestion_hint}"), self.word_span)
            }
        };
        Some(Finding::new(message, span))
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
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
    fn flags_directive_after_first_stmt() {
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

    #[test]
    fn ignores_reserved_space_comment() {
        let diags = run("--! note\nlocal _x = 1");
        assert!(diags.is_empty(), "Luau reserves `--! ` for plain comments: {diags:?}");
    }

    #[test]
    fn ignores_reserved_space_comment_after_code() {
        let diags = run("local _x = 1\n--! note");
        assert!(diags.is_empty(), "Luau reserves `--! ` for plain comments: {diags:?}");
    }
}
