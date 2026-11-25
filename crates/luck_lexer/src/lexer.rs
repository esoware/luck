mod batch;
mod number;
mod string;

use std::borrow::Cow;
use std::mem;

use luck_diagnostics::Diagnostic;
use luck_simd::DynCpu;
use luck_token::{
    Comment, CommentContent, CommentKind, CommentPosition, Span, Token, TokenKind, has_line_break,
    has_shebang,
};

use crate::cursor::Cursor;
use crate::lex_result::LexResult;
use batch::{BUFFER_TOKENS, UNREAD_TOKEN};
use string::LongBracketKind;

/// The densest real file among the benchmark files runs about this many
/// bytes per token, so most sources fit the first reservation, and pages a
/// sparser one leaves untouched cost nothing.
const BYTES_PER_TOKEN: usize = 4;

/// Keeps a huge comment-only file from reserving gigabytes it never touches.
const MAX_RESERVED_TOKENS: usize = 1 << 20;

/// A source over 4 GiB yields only the error [`Lexer::new`] reports, with no
/// tokens.
pub fn lex(source: &[u8]) -> LexResult {
    let mut lexer = match Lexer::new(source) {
        Ok(lexer) => lexer,
        Err(error) => return LexResult { errors: vec![error], ..LexResult::default() },
    };
    let mut tokens = Vec::with_capacity(lexer.token_capacity());
    if matches!(lexer.cpu, DynCpu::Generic(_)) {
        loop {
            let token = lexer.scan_token();
            tokens.push(token);
            if token.kind == TokenKind::Eof {
                break;
            }
        }
    } else {
        loop {
            lexer.refill();
            let batch = &lexer.buffer[..lexer.buffered];
            tokens.extend_from_slice(batch);
            if batch.last().is_some_and(|token| token.kind == TokenKind::Eof) {
                break;
            }
        }
    }
    lexer.finish(tokens)
}

#[derive(Debug)]
pub struct Lexer<'source> {
    cursor: Cursor<'source>,
    /// Whether a line break came since the last token. The start of the
    /// source counts as one.
    has_line_break: bool,
    /// Only read once a token has been lexed.
    last_token_start: u32,
    comments: Vec<Comment>,
    /// How many of `comments` know their token. The rest lead the next one.
    attached_comment_count: usize,
    errors: Vec<Diagnostic>,
    /// Unmatched `{` count for each open interpolation expression, innermost
    /// last. A `}` at depth 0 closes the expression and resumes the string.
    interpolation_brace_depths: Vec<u32>,
    /// Whether a block comment ran to the end of the source without its
    /// closer, which the `Eof` token after it says.
    is_in_block_comment: bool,
    /// Whether the string being lexed holds an escape, which its token says.
    has_escape: bool,
    lenient_escapes: Vec<Span>,
    /// A SIMD feature set lexes into `buffer` a batch at a time, and
    /// `Generic` a token at a time.
    cpu: DynCpu,
    buffer: [Token; BUFFER_TOKENS],
    /// How many tokens `buffer` holds, of which the first `taken` are read.
    buffered: usize,
    taken: usize,
}

impl<'source> Lexer<'source> {
    /// A lexer of `source` up to its first NUL, where Luau's lexer stops,
    /// with a shebang first line already captured as a comment.
    ///
    /// # Errors
    ///
    /// Returns an E008 for a source over 4 GiB, which `u32` spans can't
    /// address.
    pub fn new(source: &'source [u8]) -> Result<Self, Diagnostic> {
        Self::with_cpu(source, DynCpu::detect())
    }

    /// [`Self::new`] with the feature set `cpu`, which a test picks so it
    /// checks each the CPU has.
    ///
    /// # Errors
    ///
    /// Returns an E008 for a source over 4 GiB, which `u32` spans can't
    /// address.
    pub fn with_cpu(source: &'source [u8], cpu: DynCpu) -> Result<Self, Diagnostic> {
        if u32::try_from(source.len()).is_err() {
            return Err(Diagnostic::e008(
                Span::new(0, 0),
                format!(
                    "input is {} bytes, but the maximum supported file size is 4 GiB",
                    source.len()
                ),
            ));
        }

        let source_end = memchr::memchr(0, source).unwrap_or(source.len());
        let mut lexer = Self {
            cursor: Cursor::new(&source[..source_end]),
            has_line_break: true,
            last_token_start: 0,
            comments: Vec::new(),
            attached_comment_count: 0,
            errors: Vec::new(),
            interpolation_brace_depths: Vec::new(),
            is_in_block_comment: false,
            has_escape: false,
            lenient_escapes: Vec::new(),
            cpu,
            buffer: [UNREAD_TOKEN; BUFFER_TOKENS],
            buffered: 0,
            taken: 0,
        };

        if has_shebang(lexer.cursor.rest()) {
            lexer.lex_shebang();
        }

        Ok(lexer)
    }

    /// Lex and return the next token on demand, without materializing the
    /// whole stream. Comments, errors, and the rest accumulate on the lexer;
    /// collect them with [`Lexer::finish`] once the caller has drained to
    /// EOF.
    /// Returns the same EOF token forever once the source is exhausted.
    ///
    /// Always inlined, so a caller runs its loop without a call per token.
    /// Call it from one place, as the parser does.
    #[expect(
        clippy::inline_always,
        reason = "measured: the parser runs 2-4% faster for it, and `lex` 10-15%"
    )]
    #[inline(always)]
    pub fn next_token(&mut self) -> Token {
        if matches!(self.cpu, DynCpu::Generic(_)) {
            return self.scan_token();
        }
        if self.taken == self.buffered {
            self.refill();
        }
        let token = self.buffer[self.taken];
        self.taken += 1;
        token
    }

    /// Lex the next token a byte at a time, as `Generic` lexes every token
    /// and a SIMD feature set every token a window can't.
    #[expect(clippy::inline_always, reason = "inlined into `next_token`, as it was")]
    #[inline(always)]
    fn scan_token(&mut self) -> Token {
        loop {
            self.skip_whitespace();
            let start = self.cursor.position();
            // The byte after the first reads as NUL past the end of the
            // source, which no token goes on with, so one end check covers
            // both.
            let (byte, next) = match *self.cursor.rest() {
                [byte, next, ..] => (byte, next),
                [byte] => (byte, 0),
                [] => {
                    let mut eof = self.token(TokenKind::Eof, start);
                    eof.is_unterminated = self.is_in_block_comment;
                    self.has_line_break = eof.has_line_break_before;
                    return eof;
                }
            };
            // A name's scan takes its first byte too, so it vouches for the
            // whole name.
            if matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'_') {
                let name = self.cursor.eat_name();
                let kind = TokenKind::keyword(name).unwrap_or(TokenKind::Identifier);
                return self.token(kind, start);
            }
            self.cursor.bump();
            // The bytes that start most tokens come first. The rest go to
            // `lex_operator`, so this stays a small function.
            let kind = match byte {
                b'0'..=b'9' => match self.lex_number(start, byte) {
                    Some(kind) => kind,
                    None => continue,
                },
                b'(' => TokenKind::LeftParen,
                b')' => TokenKind::RightParen,
                b',' => TokenKind::Comma,
                b'=' => self.either(next, b'=', TokenKind::EqualEqual, TokenKind::Equal),
                b'.' => match next {
                    b'0'..=b'9' => match self.lex_number(start, byte) {
                        Some(kind) => kind,
                        None => continue,
                    },
                    b'.' => {
                        self.cursor.bump();
                        if self.cursor.eat(b'.') {
                            TokenKind::DotDotDot
                        } else {
                            let following = self.cursor.peek().unwrap_or(0);
                            self.either(following, b'=', TokenKind::DotDotEqual, TokenKind::DotDot)
                        }
                    }
                    _ => TokenKind::Dot,
                },
                b'"' | b'\'' => return self.lex_short_string(start, byte),
                b'{' => {
                    if let Some(depth) = self.interpolation_brace_depths.last_mut() {
                        *depth += 1;
                    }
                    TokenKind::LeftBrace
                }
                b'}' => match self.interpolation_brace_depths.last_mut() {
                    // A `}` at depth 0 closes the expression and resumes the string.
                    Some(0) => return self.lex_interpolated_string_continuation(start),
                    Some(depth) => {
                        *depth -= 1;
                        TokenKind::RightBrace
                    }
                    None => TokenKind::RightBrace,
                },
                b'[' => match self.eat_long_bracket_level() {
                    Some(level) => return self.lex_long_string(start, level),
                    None => TokenKind::LeftBracket,
                },
                b']' => TokenKind::RightBracket,
                b':' => self.either(next, b':', TokenKind::DoubleColon, TokenKind::Colon),
                b'-' if next == b'-' => {
                    self.lex_comment(start);
                    continue;
                }
                b'`' => return self.lex_interpolated_string(start),
                _ => match self.lex_operator(start, byte, next) {
                    Some(kind) => kind,
                    None => continue,
                },
            };
            return self.token(kind, start);
        }
    }

    /// What a list of the tokens left to lex reserves.
    pub fn token_capacity(&self) -> usize {
        self.cursor.rest().len().div_ceil(BYTES_PER_TOKEN).min(MAX_RESERVED_TOKENS)
    }

    /// What the lexer gathered, with `tokens`, the ones its caller took from
    /// [`Lexer::next_token`].
    pub fn finish(self, tokens: Vec<Token>) -> LexResult {
        LexResult {
            tokens,
            comments: self.comments,
            errors: self.errors,
            lenient_escapes: self.lenient_escapes,
        }
    }

    #[inline]
    fn skip_whitespace(&mut self) {
        self.cursor.eat_while(|byte| match byte {
            b' ' | b'\t' | b'\r' | 0x0B | 0x0C => true,
            b'\n' => {
                self.has_line_break = true;
                true
            }
            _ => false,
        });
    }

    /// The token from `start` to the cursor, which also leads every comment
    /// still waiting for one.
    #[inline]
    fn token(&mut self, kind: TokenKind, start: usize) -> Token {
        let span = self.cursor.span_since(start);
        if self.attached_comment_count < self.comments.len() {
            self.attach_leading_comments(span.start);
        }
        self.last_token_start = span.start;
        Token {
            span,
            kind,
            has_line_break_before: mem::replace(&mut self.has_line_break, false),
            is_unterminated: false,
            has_escape: mem::take(&mut self.has_escape),
        }
    }

    fn attach_leading_comments(&mut self, token_start: u32) {
        for comment in &mut self.comments[self.attached_comment_count..] {
            comment.attached_to = token_start;
        }
        self.attached_comment_count = self.comments.len();
    }

    #[cold]
    fn reject(&mut self, start: usize, message: impl Into<Cow<'static, str>>) {
        self.errors.push(Diagnostic::e008(self.cursor.span_since(start), message));
    }

    fn lex_shebang(&mut self) {
        let start = self.cursor.position();
        // Luau's loader skips this line up to its LF, so a lone CR stays in
        // the comment and only a CRLF's CR is left out of it.
        let rest = self.cursor.rest();
        let line_length = memchr::memchr(b'\n', rest).unwrap_or(rest.len());
        let line = &rest[..line_length];
        self.cursor.advance_by(line.strip_suffix(b"\r").unwrap_or(line).len());
        self.comments.push(Comment {
            span: self.cursor.span_since(start),
            attached_to: 0,
            kind: CommentKind::Shebang,
            position: CommentPosition::Leading,
            is_followed_by_newline: self.cursor.is_at_line_end(),
            content: CommentContent::Plain,
        });
    }

    /// A block comment cut off before its closer is still a comment, through
    /// the end of the source. The cursor is past the first `-`.
    fn lex_comment(&mut self, start: usize) {
        self.cursor.bump();

        let kind = if self.cursor.eat(b'[')
            && let Some(level) = self.eat_long_bracket_level()
        {
            let body = self.scan_long_bracket_body(start, level, LongBracketKind::Comment);
            self.is_in_block_comment = !body.is_closed;
            // The delimiters (`--[=*[` and `]=*]`) never hold a line break,
            // so scanning the whole comment is equivalent to scanning only
            // the body.
            if has_line_break(self.cursor.text_since(start)) {
                CommentKind::MultiLineBlock
            } else {
                CommentKind::SingleLineBlock
            }
        } else {
            self.cursor.advance_to_line_break();
            CommentKind::Line
        };

        // A comment with no line break since the previous token trails it,
        // and any other leads the next token. The start of the source counts
        // as a line break, so a trailing comment always has a token.
        let (attached_to, position) = if self.has_line_break {
            (0, CommentPosition::Leading)
        } else {
            (self.last_token_start, CommentPosition::Trailing)
        };
        self.comments.push(Comment {
            span: self.cursor.span_since(start),
            attached_to,
            kind,
            position,
            is_followed_by_newline: self.cursor.is_at_line_end(),
            content: CommentContent::of(self.cursor.text_since(start), kind),
        });
        if position == CommentPosition::Trailing {
            self.attached_comment_count = self.comments.len();
        }
        if kind == CommentKind::MultiLineBlock {
            self.has_line_break = true;
        }
    }

    /// Lex an operator from its first `byte` at `start`, with the cursor past
    /// it, at `next`, or report the byte and return `None` where it starts no
    /// token.
    fn lex_operator(&mut self, start: usize, byte: u8, next: u8) -> Option<TokenKind> {
        Some(match byte {
            b';' => TokenKind::Semicolon,
            b'#' => TokenKind::Hash,
            b'-' => {
                if next == b'>' {
                    self.cursor.bump();
                    TokenKind::Arrow
                } else {
                    self.either(next, b'=', TokenKind::MinusEqual, TokenKind::Minus)
                }
            }
            b'+' => self.either(next, b'=', TokenKind::PlusEqual, TokenKind::Plus),
            b'*' => self.either(next, b'=', TokenKind::StarEqual, TokenKind::Star),
            b'/' => {
                if next == b'/' {
                    self.cursor.bump();
                    let following = self.cursor.peek().unwrap_or(0);
                    self.either(following, b'=', TokenKind::FloorDivEqual, TokenKind::FloorDiv)
                } else {
                    self.either(next, b'=', TokenKind::SlashEqual, TokenKind::Slash)
                }
            }
            b'%' => self.either(next, b'=', TokenKind::PercentEqual, TokenKind::Percent),
            b'^' => self.either(next, b'=', TokenKind::CaretEqual, TokenKind::Caret),
            b'<' => self.either(next, b'=', TokenKind::LessEqual, TokenKind::Less),
            b'>' => self.either(next, b'=', TokenKind::GreaterEqual, TokenKind::Greater),
            b'~' => {
                if next != b'=' {
                    let span = self.cursor.span_since(start);
                    let error = Diagnostic::e008(span, "unexpected character `~`")
                        .with_help("use `~=` for not-equal");
                    self.errors.push(error);
                    return None;
                }
                self.cursor.bump();
                TokenKind::TildeEqual
            }
            b'&' => TokenKind::Ampersand,
            b'|' => TokenKind::Pipe,
            b'@' => TokenKind::At,
            b'?' => TokenKind::Question,
            _ => {
                self.reject_unexpected_character(start, byte);
                return None;
            }
        })
    }

    /// `pair` when `next`, the byte at the cursor, is `second`, consuming it,
    /// else `single`.
    #[inline]
    fn either(&mut self, next: u8, second: u8, pair: TokenKind, single: TokenKind) -> TokenKind {
        if next == second {
            self.cursor.bump();
            pair
        } else {
            single
        }
    }

    #[cold]
    fn reject_unexpected_character(&mut self, start: usize, byte: u8) {
        let message = match self.cursor.eat_rest_of_char(start) {
            Some(unexpected) => format!("unexpected character `{unexpected}`"),
            None => format!("unexpected byte 0x{byte:02X}"),
        };
        self.reject(start, message);
    }
}
