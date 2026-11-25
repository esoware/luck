use std::borrow::Cow;

use luck_diagnostics::Diagnostic;
use luck_token::{Span, TokenKind};

use super::Parser;
use super::forgotten::UnclosedBlock;
use super::stmt::is_stmt_start;

/// A token that closes a construct, or continues it, as `else` does an `if`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Closer {
    Paren,
    Bracket,
    Brace,
    /// An interpolated string's next segment, after an expression.
    StringSegment,
    /// `end` or `until`, each of which ends any block, as in every Lua's
    /// parser, so a mistaken one ends the block it's in, and the construct
    /// around that block reports what it was missing.
    BlockEnd,
    /// `else` or `elseif`.
    Else,
}

impl Closer {
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token closes nothing")]
    const fn of(kind: TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::RightParen => Self::Paren,
            TokenKind::RightBracket => Self::Bracket,
            TokenKind::RightBrace => Self::Brace,
            TokenKind::InterpolatedStringMiddle | TokenKind::InterpolatedStringEnd => {
                Self::StringSegment
            }
            TokenKind::End | TokenKind::Until => Self::BlockEnd,
            TokenKind::Else | TokenKind::ElseIf => Self::Else,
            _ => return None,
        })
    }
}

/// How many constructs the parser is inside that each closer closes, so
/// recovery can tell a closer something open expects from a stray one.
#[derive(Clone, Copy, Default)]
pub(super) struct OpenConstructs {
    parens: u32,
    brackets: u32,
    braces: u32,
    string_segments: u32,
    block_ends: u32,
    elses: u32,
}

impl OpenConstructs {
    const fn count_mut(&mut self, closer: Closer) -> &mut u32 {
        match closer {
            Closer::Paren => &mut self.parens,
            Closer::Bracket => &mut self.brackets,
            Closer::Brace => &mut self.braces,
            Closer::StringSegment => &mut self.string_segments,
            Closer::BlockEnd => &mut self.block_ends,
            Closer::Else => &mut self.elses,
        }
    }

    const fn expects(mut self, closer: Closer) -> bool {
        *self.count_mut(closer) > 0
    }
}

impl Parser<'_> {
    /// Whether a syntax error was reported, or a string or block comment cut
    /// off before its closer passed, since the last statement began where
    /// one reliably does. Errors that follow either in the statement would
    /// only echo it: a cut-off string took the rest of its line, and a
    /// cut-off comment the rest of the source, often with the closer of what
    /// holds it, so only `Eof` follows that.
    #[inline]
    pub(super) fn is_recovering(&self) -> bool {
        self.has_syntax_error || (self.token.is_unterminated && self.is_at_eof())
    }

    /// Count a construct `closer` closes as open from here.
    pub(super) const fn open(&mut self, closer: Closer) {
        *self.open_constructs.count_mut(closer) += 1;
    }

    /// Count the construct `closer` closes as closed from here.
    pub(super) const fn close(&mut self, closer: Closer) {
        *self.open_constructs.count_mut(closer) -= 1;
    }

    /// Close the construct `closer` closes and consume its `kind` closer, as
    /// [`Self::expect`] does, but one that's missing is an error that points
    /// at `opener` too, the token that opened the construct.
    #[inline]
    pub(super) fn expect_closer(&mut self, kind: TokenKind, closer: Closer, opener: Span) -> Span {
        self.close(closer);
        if self.is_at(kind) { self.advance() } else { self.report_missing_closer(kind, opener) }
    }

    /// A closing bracket of another kind that nothing open expects is a typo
    /// for a missing bracket, so it closes the bracket too.
    #[cold]
    #[inline(never)]
    fn report_missing_closer(&mut self, kind: TokenKind, opener: Span) -> Span {
        let span = self.report_unclosed(kind, opener);
        let is_bracket = |closer| {
            matches!(
                closer,
                TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
            )
        };
        if is_bracket(kind) && is_bracket(self.peek()) && !self.is_at_expected_closer() {
            self.advance();
        }
        span
    }

    /// Record that the current token isn't the `kind` that closes what
    /// `opener` opened, as PUC Lua and Luau report it, with the opener
    /// labeled. Leaves the token in place and returns its span.
    #[cold]
    #[inline(never)]
    pub(super) fn report_unclosed(&mut self, kind: TokenKind, opener: Span) -> Span {
        let span = self.token.span;
        if !self.is_recovering() {
            self.has_syntax_error = true;
            let opener_text = String::from_utf8_lossy(opener.text(self.source));
            let message = format!(
                "expected {kind} to close `{opener_text}`, found {}",
                self.token.display(self.source)
            );
            let diag = Diagnostic::e008(span, message)
                .with_label(opener, format!("unclosed `{opener_text}`"));
            if matches!(kind, TokenKind::End | TokenKind::Until) {
                self.unclosed_blocks.push(UnclosedBlock { error_index: self.errors.len(), opener });
            }
            self.errors.push(diag);
        }
        span
    }

    /// Whether the current token closes or continues something open.
    pub(super) fn is_at_expected_closer(&self) -> bool {
        Closer::of(self.peek()).is_some_and(|closer| self.open_constructs.expects(closer))
    }

    /// Whether the block being parsed ends at the current token.
    pub(super) fn is_at_block_end(&self) -> bool {
        self.is_at_eof() || self.is_at_expected_closer()
    }

    /// The message is only built when it's reported.
    #[cold]
    #[inline(never)]
    pub(super) fn report_expected(&mut self, kind: TokenKind) -> Span {
        let span = self.token.span;
        if !self.is_recovering() {
            self.has_syntax_error = true;
            let message = format!("expected {kind}, found {}", self.token.display(self.source));
            self.error(span, message);
        }
        span
    }

    #[cold]
    #[inline(never)]
    pub(super) fn error<Message>(&mut self, span: Span, message: Message)
    where
        Message: Into<Cow<'static, str>>,
    {
        self.errors.push(Diagnostic::e008(span, message));
    }

    /// Record a syntax error unless one is already being recovered from.
    #[cold]
    #[inline(never)]
    pub(super) fn syntax_error<Message>(&mut self, span: Span, message: Message)
    where
        Message: Into<Cow<'static, str>>,
    {
        if !self.is_recovering() {
            self.has_syntax_error = true;
            self.error(span, message);
        }
    }

    /// Skip to the block's end, or to where a statement reliably begins and
    /// recovery ends. False when it skipped, so the block looks again.
    #[cold]
    #[inline(never)]
    pub(super) fn recover_at_statement(&mut self) -> bool {
        if self.is_at_block_end() {
            return true;
        }
        if !self.is_at_reliable_stmt_start() {
            self.skip_to_statement();
            return false;
        }
        self.has_syntax_error = false;
        true
    }

    /// Skip to where a statement reliably begins, or to a closer something
    /// open expects. A name or a `(` on the line of an error may be what's
    /// left of the statement it broke, so one begins a statement only at the
    /// start of a line.
    fn skip_to_statement(&mut self) {
        while !self.is_at_block_end() && !self.is_at_reliable_stmt_start() {
            self.advance();
        }
    }

    const fn is_at_reliable_stmt_start(&self) -> bool {
        let kind = self.peek();
        is_stmt_start(kind)
            && (!(kind.is_name() || matches!(kind, TokenKind::LeftParen))
                || self.token.has_line_break_before)
    }
}
