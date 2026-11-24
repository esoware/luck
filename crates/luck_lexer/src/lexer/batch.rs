use luck_simd::DynCpu;
use luck_token::{Keywords, Span, Token, TokenKind};

use super::Lexer;
use crate::window::{Classify, Planes, Tables, WINDOW_BYTES, WindowBytes};

/// A batch's tokens, which stay in the cache while the parser reads them.
pub(super) const BUFFER_TOKENS: usize = 256;

pub(super) const UNREAD_TOKEN: Token = Token {
    span: Span::new(0, 0),
    kind: TokenKind::Eof,
    has_line_break_before: false,
    is_unterminated: false,
    has_escape: false,
};

impl Lexer<'_> {
    /// Replace `buffer` with the next batch of tokens, through the last if
    /// the source ends.
    #[inline(never)]
    pub(super) fn refill(&mut self) {
        match self.cpu {
            DynCpu::Avx2(cpu) => cpu.run(|cpu| self.fill_buffer(cpu)),
            DynCpu::Neon(cpu) => self.fill_buffer(cpu),
            DynCpu::Generic(_) => unreachable!("`Generic` lexes a token at a time"),
        }
    }

    /// Lex a window of simple tokens at once wherever one can, and each other
    /// token with `scan_token`, until the buffer has no room for another
    /// window's tokens or holds the `Eof`.
    #[expect(
        clippy::inline_always,
        reason = "inlined into `refill`, which enables the feature set"
    )]
    #[inline(always)]
    fn fill_buffer(&mut self, cpu: impl Classify) {
        self.buffered = 0;
        self.taken = 0;
        let tables = Tables::of(self.version);
        let keywords = Keywords::of(self.version);
        while self.buffered + WINDOW_BYTES < BUFFER_TOKENS {
            // A comment waiting for its token, or an interpolation's braces,
            // which `scan_token` counts, take the scalar lexer.
            if self.attached_comment_count == self.comments.len()
                && self.interpolation_brace_depths.is_empty()
            {
                let base = self.cursor.position();
                let bytes = window_bytes(self.cursor.rest());
                let (head, _) = bytes.split_first_chunk().expect("72 bytes");
                let planes = cpu.classify(head, tables);
                if !self.emit_window(base, &bytes, planes, keywords) {
                    continue;
                }
            }
            let token = self.scan_complex_token();
            self.buffer[self.buffered] = token;
            self.buffered += 1;
            if token.kind == TokenKind::Eof {
                return;
            }
        }
    }

    #[inline(never)]
    fn scan_complex_token(&mut self) -> Token {
        self.scan_token()
    }

    /// Emit the simple tokens that open the window at `base`, the cursor:
    /// names, keywords, decimal integers that touch nothing, and punctuation
    /// that nothing joins, each spelled as `scan_token` reads it. Returns
    /// whether the next token needs `scan_token`, because it's another kind
    /// or the window ended inside it.
    #[expect(clippy::inline_always, reason = "the batch loop's body")]
    #[inline(always)]
    fn emit_window(
        &mut self,
        base: usize,
        bytes: &WindowBytes,
        planes: Planes,
        keywords: &Keywords,
    ) -> bool {
        let Planes { whitespace, name, digit, line_break, punct, continuation, dot } = planes;
        // The window opens at the cursor, where no name runs on from before.
        let name_starts = name & !(name << 1);
        let digit_starts = name_starts & digit;
        // A carry from each run's first byte clears the run and sets the
        // byte past it, which leaves every other run alone.
        let past_digit_runs = name.wrapping_add(digit_starts);
        let malformed_numerals =
            (name & !past_digit_runs & !digit) | (past_digit_runs & !name & dot);
        let mut complex =
            !(whitespace | name | punct) | (punct & (continuation >> 1)) | (dot & (digit >> 1));
        if malformed_numerals != 0 {
            let first = malformed_numerals.trailing_zeros();
            let numerals_up_to_first = digit_starts & (u64::MAX >> (63 - first));
            complex |= 1 << numerals_up_to_first.ilog2();
        }
        let starts = name_starts | punct;
        // A name or punctuation in the last byte may go on past the window.
        let limit = if (name | punct) >> 63 == 0 { 64 } else { starts.ilog2() };
        let first_complex = complex.trailing_zeros();
        let end = first_complex.min(limit);

        let mut pending = starts & below(end);
        let mut has_line_break = self.has_line_break;
        let mut line_breaks_passed = 0;
        let mut last_start = None;
        while pending != 0 {
            let offset = pending.trailing_zeros();
            pending &= pending - 1;
            // Only whitespace separates these tokens, and none holds a line
            // break, so a new line break below this one is in its gap.
            let line_breaks_below = line_break & ((1 << offset) - 1);
            let has_line_break_before = has_line_break || line_breaks_below != line_breaks_passed;
            has_line_break = false;
            line_breaks_passed = line_breaks_below;

            let index = offset as usize;
            let name_length = (!(name >> offset)).trailing_zeros() as usize;
            let (first_eight, _) =
                bytes[index..].split_first_chunk::<8>().expect("8 bytes of slack");
            let kind = if name_length == 0 {
                punct_kind(bytes[index])
            } else if (digit >> offset) & 1 == 1 {
                TokenKind::Number
            } else {
                keywords.kind(u64::from_le_bytes(*first_eight), name_length)
            };
            let start = base + index;
            self.buffer[self.buffered] = Token {
                span: span(start, start + name_length.max(1)),
                kind,
                has_line_break_before,
                is_unterminated: false,
                has_escape: false,
            };
            self.buffered += 1;
            last_start = Some(start);
        }
        self.has_line_break = has_line_break || line_break & below(end) != line_breaks_passed;
        if let Some(start) = last_start {
            self.last_token_start = span(start, start).start;
        }
        self.cursor.advance_by(end as usize);
        end == 0 || first_complex <= limit
    }
}

/// The bits below bit `count`.
#[inline]
fn below(count: u32) -> u64 {
    u64::MAX.checked_shr(64 - count).unwrap_or(0)
}

/// A plain cast, as the cursor's spans are.
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "`Lexer::new` refuses a source over `u32::MAX` bytes, so a position fits a `u32`"
)]
fn span(start: usize, end: usize) -> Span {
    Span::new(start as u32, end as u32)
}

#[inline]
fn window_bytes(rest: &[u8]) -> WindowBytes {
    match rest.first_chunk() {
        Some(bytes) => *bytes,
        None => tail_window_bytes(rest),
    }
}

/// Only the last windows of a source run past its end.
#[cold]
#[inline(never)]
fn tail_window_bytes(rest: &[u8]) -> WindowBytes {
    let mut bytes = [0; WINDOW_BYTES + 8];
    bytes[..rest.len()].copy_from_slice(rest);
    bytes
}

/// The token a punctuation byte that no continuation follows spells alone.
#[inline]
const fn punct_kind(byte: u8) -> TokenKind {
    match byte {
        b'#' => TokenKind::Hash,
        b'%' => TokenKind::Percent,
        b'&' => TokenKind::Ampersand,
        b'(' => TokenKind::LeftParen,
        b')' => TokenKind::RightParen,
        b'*' => TokenKind::Star,
        b'+' => TokenKind::Plus,
        b',' => TokenKind::Comma,
        b'-' => TokenKind::Minus,
        b'.' => TokenKind::Dot,
        b'/' => TokenKind::Slash,
        b':' => TokenKind::Colon,
        b';' => TokenKind::Semicolon,
        b'<' => TokenKind::Less,
        b'=' => TokenKind::Equal,
        b'>' => TokenKind::Greater,
        b'?' => TokenKind::Question,
        b'@' => TokenKind::At,
        b'[' => TokenKind::LeftBracket,
        b']' => TokenKind::RightBracket,
        b'^' => TokenKind::Caret,
        b'{' => TokenKind::LeftBrace,
        b'|' => TokenKind::Pipe,
        b'}' => TokenKind::RightBrace,
        b'~' => TokenKind::Tilde,
        _ => TokenKind::Identifier,
    }
}
