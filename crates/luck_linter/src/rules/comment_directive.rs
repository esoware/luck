use luck_token::{CommentContent, Span, is_lua_whitespace};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};
use crate::suggest::closest;

const KNOWN_DIRECTIVES: &[&str] =
    &["nolint", "nocheck", "nonstrict", "strict", "optimize", "native"];

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CommentDirective;

declare_lint_rule!(
    CommentDirective,
    Correctness,
    Warning,
    "invalid or misplaced `--!` comment directive"
);

impl Rule for CommentDirective {
    fn run_once(&self, context: &LintContext<'_>) {
        let first_token_start = context.chunk.block.span.start;
        let mut first_mode: Option<Span> = None;
        for comment in context.comments {
            if comment.content != CommentContent::HotComment {
                continue;
            }
            // Luau ignores hot comments after the first token.
            if comment.span.start > first_token_start {
                context.diagnostic(Finding::new(
                    "comment directive after the first statement has no effect",
                    comment.span,
                ));
                continue;
            }
            let body = comment.span.text(context.source).get(3..).unwrap_or_default();
            let hot_comment = HotComment::split(body, comment.span.start.saturating_add(3));
            if !hot_comment.sets_mode() {
                if let Some(finding) = hot_comment.problem() {
                    context.diagnostic(finding);
                }
                continue;
            }
            match first_mode {
                Some(first) => context.diagnostic(
                    Finding::new(
                        "type checking mode is already set by an earlier directive",
                        hot_comment.word_span,
                    )
                    .with_label(first, "first set here"),
                ),
                None => first_mode = Some(hot_comment.word_span),
            }
        }
    }
}

struct HotComment<'body> {
    word: &'body [u8],
    rest: &'body [u8],
    word_span: Span,
}

impl<'body> HotComment<'body> {
    fn split(body: &'body [u8], word_start: u32) -> Self {
        let word_length = body.iter().position(|&b| is_lua_whitespace(b)).unwrap_or(body.len());
        let (word, rest) = body.split_at_checked(word_length).unwrap_or((body, &[]));
        Self { word, rest, word_span: Span::from_indices(0, word_length).shifted(word_start) }
    }

    fn trailing_symbols_span(&self) -> Option<Span> {
        let first = self.rest.iter().position(|&b| !is_lua_whitespace(b))?;
        let last = self.rest.iter().rposition(|&b| !is_lua_whitespace(b))?;
        Some(Span::from_indices(first, last.saturating_add(1)).shifted(self.word_span.end))
    }

    const fn is_mode(&self) -> bool {
        matches!(self.word, b"nocheck" | b"nonstrict" | b"strict")
    }

    fn sets_mode(&self) -> bool {
        self.is_mode() && self.trailing_symbols_span().is_none()
    }

    /// What's wrong with this directive, short of repeating an earlier mode.
    fn problem(&self) -> Option<Finding> {
        let (message, span) = match self.word {
            // `nolint` arguments name Luau's lints, not luck's.
            b"nolint" => return None,
            b"native" => (
                "`native` directive has extra symbols at the end of the line".to_owned(),
                self.trailing_symbols_span()?,
            ),
            _ if self.is_mode() => (
                "type checking mode directive has extra symbols at the end of the line".to_owned(),
                self.trailing_symbols_span()?,
            ),
            b"optimize" => {
                let Some(level_span) = self.trailing_symbols_span() else {
                    return Some(Finding::new(
                        "`optimize` directive requires an optimization level",
                        self.word_span,
                    ));
                };
                let level = self.rest.trim_ascii();
                if matches!(level, b"0" | b"1" | b"2") {
                    return None;
                }
                let shown = String::from_utf8_lossy(level);
                (
                    format!("unknown optimization level `{shown}`, expected `0`, `1`, or `2`"),
                    level_span,
                )
            }
            unknown => {
                let word = String::from_utf8_lossy(unknown);
                let suggestion_hint =
                    closest(&word, KNOWN_DIRECTIVES.iter().copied(), |distance, _| distance <= 2)
                        .map_or_else(String::new, |suggestion| {
                            format!(", did you mean `{suggestion}`?")
                        });
                (format!("unknown comment directive `{word}`{suggestion_hint}"), self.word_span)
            }
        };
        Some(Finding::new(message, span))
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::CommentDirective, source, LuaVersion::Luau)
    }

    #[test]
    fn flags_unknown_directive() {
        let diags = run("\
--!foobar
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("unknown comment directive `foobar`"));
        assert!(!diags[0].message.contains("did you mean"));
    }

    #[test]
    fn flags_unknown_directive_with_suggestion() {
        let diags = run("\
--!strcit
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("did you mean `strict`"), "{diags:?}");
    }

    #[test]
    fn flags_directive_after_first_stmt() {
        let diags = run("\
local _x = 1
--!strict");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("after the first statement"));
    }

    #[test]
    fn flags_directive_after_lone_return() {
        let diags = run("\
return 1
--!strict");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("after the first statement"));
    }

    #[test]
    fn flags_extra_symbols_after_mode() {
        let diags = run("\
--!strict yes
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("extra symbols"));
    }

    #[test]
    fn flags_second_mode_directive() {
        let diags = run("\
--!strict
--!nocheck
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("set by an earlier directive"));
    }

    #[test]
    fn flags_optimize_without_level() {
        let diags = run("\
--!optimize
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("requires an optimization level"));
    }

    #[test]
    fn flags_optimize_bad_level() {
        let diags = run("\
--!optimize 3
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].message.contains("unknown optimization level `3`, expected `0`, `1`, or `2`"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_extra_symbols_after_native() {
        let diags = run("\
--!native on
local _x = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("extra symbols"));
    }

    #[test]
    fn ignores_valid_directives() {
        let diags = run("\
--!strict
--!native
--!optimize 2
local _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_nolint_with_argument() {
        let diags = run("\
--!nolint UnknownGlobal
local _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_plain_comments() {
        let diags = run("\
-- not a directive
local _x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_luau() {
        let diags = run_rule(
            RuleId::CommentDirective,
            "\
--!foobar
local _x = 1",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_reserved_space_comment() {
        let diags = run("\
--! note
local _x = 1");
        assert!(diags.is_empty(), "Luau reserves `--! ` for plain comments: {diags:?}");
    }

    #[test]
    fn ignores_reserved_space_comment_after_code() {
        let diags = run("\
local _x = 1
--! note");
        assert!(diags.is_empty(), "Luau reserves `--! ` for plain comments: {diags:?}");
    }
}
