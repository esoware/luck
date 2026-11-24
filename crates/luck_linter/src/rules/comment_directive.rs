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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        (
            "\
--!strict
--!native
--!optimize 2
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
--!nolint UnknownGlobal
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
-- not a directive
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
--!foobar
local _x = 1",
            LuaTarget::Lua54,
        ),
        // Luau reserves `--! ` for plain comments.
        (
            "\
--! note
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
local _x = 1
--! note",
            LuaTarget::Luau,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (
            "\
--!foobar
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
--!strcit
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
local _x = 1
--!strict",
            LuaTarget::Luau,
        ),
        (
            "\
return 1
--!strict",
            LuaTarget::Luau,
        ),
        (
            "\
--!strict yes
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
--!strict
--!nocheck
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
--!optimize
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
--!optimize 3
local _x = 1",
            LuaTarget::Luau,
        ),
        (
            "\
--!native on
local _x = 1",
            LuaTarget::Luau,
        ),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::CommentDirective, PASS, FAIL).test_and_snapshot();
    }
}
