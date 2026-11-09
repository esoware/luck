use luck_token::{
    Comment, CommentKind, CommentPosition, CompactString, LuaVersion, Span, Token, TokenKind,
    chunk_start, has_line_break,
};

use crate::cursor::Cursor;
use crate::lex_error::{LexError, lex_error};
use crate::lex_result::LexResult;
use crate::number::scan_number;
use crate::string::{
    InterpolatedSegmentEnd, advance_past_long_bracket_close, eat_long_bracket_open,
    scan_interpolated_segment, scan_long_string, scan_short_string,
};

/// The median file in the bench corpus runs about this many bytes per token.
const BYTES_PER_TOKEN: usize = 5;

/// Keeps a huge comment-only file from reserving gigabytes it never touches.
const MAX_RESERVED_TOKENS: usize = 1 << 20;

/// Lex all of `source` at once.
///
/// A source over 4 GiB yields only the error [`Lexer::new`] reports, with no
/// tokens.
#[must_use]
pub fn lex(source: &str, version: LuaVersion) -> LexResult {
    match Lexer::new(source, version) {
        Ok(lexer) => lexer.tokenize(),
        Err(error) => LexResult { tokens: Vec::new(), comments: Vec::new(), errors: vec![error] },
    }
}

#[derive(Debug)]
pub struct Lexer<'source> {
    cursor: Cursor<'source>,
    version: LuaVersion,
    /// Second token of a double emit, handed out first on the next
    /// `next_token` call. An interpolated string with no expressions produces
    /// `InterpolatedStringBegin` + `InterpolatedStringEnd` from one dispatch.
    queued: Option<Token>,
    comments: Vec<Comment>,
    errors: Vec<LexError>,
    /// `None` until the first token.
    last_token_start: Option<u32>,
    /// The file start counts as a newline.
    has_newline_since_last_token: bool,
    /// Leading comments that get their `attached_to` from the next token.
    pending_leading_comments: Vec<PendingComment>,
    /// Unmatched `{` count for each open interpolation expression, innermost
    /// last (Luau). A `}` at depth 0 closes the expression and resumes the
    /// string.
    interpolation_brace_depths: Vec<u32>,
}

/// A comment whose attachment is not settled yet.
#[derive(Debug)]
struct PendingComment {
    span: Span,
    kind: CommentKind,
    is_preceded_by_newline: bool,
    is_followed_by_newline: bool,
}

impl PendingComment {
    const fn attach(self, attached_to: u32, position: CommentPosition) -> Comment {
        Comment {
            span: self.span,
            attached_to,
            kind: self.kind,
            position,
            is_preceded_by_newline: self.is_preceded_by_newline,
            is_followed_by_newline: self.is_followed_by_newline,
        }
    }
}

impl<'source> Lexer<'source> {
    /// A lexer positioned past a UTF-8 BOM `version` skips, with a shebang
    /// first line already captured as a comment.
    ///
    /// # Errors
    ///
    /// Returns a [`LexError`] for a source over 4 GiB, which `u32` spans can't
    /// address.
    pub fn new(source: &'source str, version: LuaVersion) -> Result<Self, LexError> {
        if u32::try_from(source.len()).is_err() {
            return Err(lex_error(
                Span::new(0, 0),
                format!(
                    "input is {} bytes, but the maximum supported file size is 4 GiB",
                    source.len()
                ),
            ));
        }

        let source_end = if version.has_nul_source_end() {
            memchr::memchr(0, source.as_bytes()).unwrap_or(source.len())
        } else {
            source.len()
        };

        let mut lexer = Self {
            cursor: Cursor::new(source.get(..source_end).unwrap_or(source)),
            version,
            queued: None,
            comments: Vec::new(),
            errors: Vec::new(),
            last_token_start: None,
            has_newline_since_last_token: true,
            pending_leading_comments: Vec::new(),
            interpolation_brace_depths: Vec::new(),
        };

        lexer.cursor.advance_by(chunk_start(source, version));

        let is_shebang = match lexer.cursor.rest() {
            [b'#', b'!', _, ..] => true,
            [b'#', ..] => version.has_bare_hash_shebang(),
            _ => false,
        };
        if is_shebang {
            lexer.lex_shebang();
        }

        Ok(lexer)
    }

    /// Lex and return the next token on demand, without materializing the
    /// whole stream. Comments and errors accumulate on the lexer; collect
    /// them with [`Lexer::finish`] once the caller has drained to EOF.
    /// Returns EOF tokens forever once the source is exhausted.
    pub fn next_token(&mut self) -> Token {
        if let Some(token) = self.queued.take() {
            return token;
        }
        loop {
            self.skip_whitespace();
            let Some(byte) = self.cursor.peek() else {
                let eof_start = self.cursor.position();
                return self.make_token(TokenKind::Eof, eof_start);
            };
            if let Some(token) = self.dispatch_byte(byte) {
                return token;
            }
        }
    }

    /// Consume the lexer after streaming, yielding the accumulated
    /// comments and errors.
    #[must_use]
    pub fn finish(self) -> (Vec<Comment>, Vec<LexError>) {
        (self.comments, self.errors)
    }

    /// Lex the whole source into one stream ending in EOF. Its own loop
    /// rather than `next_token` until EOF, so no per-token EOF test.
    fn tokenize(mut self) -> LexResult {
        let capacity = self.cursor.rest().len().div_ceil(BYTES_PER_TOKEN);
        let mut tokens = Vec::with_capacity(capacity.min(MAX_RESERVED_TOKENS));
        loop {
            self.skip_whitespace();
            let Some(byte) = self.cursor.peek() else {
                break;
            };
            if let Some(token) = self.dispatch_byte(byte) {
                tokens.push(token);
                if let Some(queued) = self.queued.take() {
                    tokens.push(queued);
                }
            }
        }

        let eof_start = self.cursor.position();
        tokens.push(self.make_token(TokenKind::Eof, eof_start));

        LexResult { tokens, comments: self.comments, errors: self.errors }
    }

    /// Single-jump dispatch on the lead byte. A match compiles to the same
    /// jump table as a fn-pointer array but keeps the small handlers
    /// inlinable. Callers consume whitespace first. Returns `None` when the
    /// byte produced no token (a comment or an error).
    #[inline]
    fn dispatch_byte(&mut self, byte: u8) -> Option<Token> {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => Some(self.lex_identifier()),
            b'0'..=b'9' => self.lex_number(),
            b'.' if matches!(self.cursor.peek_next(), Some(b'0'..=b'9')) => self.lex_number(),
            b'"' | b'\'' => self.lex_short_string(byte),
            b'[' => self.lex_long_string_or_bracket(),
            // Luau
            b'`' if self.version.has_interpolated_strings() => self.lex_interpolated_string(),
            // Luau: a `}` at depth 0 closes the expression and resumes the string.
            b'}' if self.interpolation_brace_depths.last() == Some(&0) => {
                self.lex_interpolated_string_continuation()
            }
            b'-' if self.cursor.peek_next() == Some(b'-') => {
                self.lex_comment();
                None
            }
            _ => self.lex_symbol(),
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.cursor.peek() {
            match byte {
                b' ' | b'\t' | 0x0B | 0x0C => {
                    self.cursor.advance();
                }
                b'\n' => {
                    self.cursor.advance();
                    self.has_newline_since_last_token = true;
                }
                b'\r' => {
                    self.cursor.advance();
                    self.has_newline_since_last_token |= self.version.has_lfcr_line_break();
                }
                _ => break,
            }
        }
    }

    #[inline]
    fn make_token(&mut self, kind: TokenKind, start: usize) -> Token {
        let span = self.cursor.span_since(start);

        for pending in self.pending_leading_comments.drain(..) {
            self.comments.push(pending.attach(span.start, CommentPosition::Leading));
        }

        self.last_token_start = Some(span.start);
        self.has_newline_since_last_token = false;
        Token::new(kind, span)
    }

    /// The value of a sub-scan, or `None` after recording its error so lexing
    /// resumes past it.
    #[inline]
    fn ok_or_record<T>(&mut self, result: Result<T, LexError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.errors.push(error);
                None
            }
        }
    }

    /// Record an error spanning `start` to the cursor and yield no token.
    #[cold]
    fn reject(&mut self, start: usize, message: impl Into<String>) -> Option<Token> {
        self.errors.push(lex_error(self.cursor.span_since(start), message));
        None
    }

    fn lex_shebang(&mut self) {
        let start = self.cursor.position();
        // Every loader skips this line up to LF, so a lone CR stays in the
        // comment and only a CRLF's CR is left out of it.
        let rest = self.cursor.rest();
        let line_length = memchr::memchr(b'\n', rest).unwrap_or(rest.len());
        let line = rest.get(..line_length).unwrap_or_default();
        self.cursor.advance_by(line.strip_suffix(b"\r").unwrap_or(line).len());
        self.pending_leading_comments.push(PendingComment {
            span: self.cursor.span_since(start),
            kind: CommentKind::Shebang,
            is_preceded_by_newline: false,
            is_followed_by_newline: self.cursor.is_at_line_end(self.version),
        });
    }

    fn lex_comment(&mut self) {
        let start = self.cursor.position();
        self.cursor.advance_by(2); // `--`

        let kind = if let Some(level) = eat_long_bracket_open(&mut self.cursor) {
            let result = advance_past_long_bracket_close(
                &mut self.cursor,
                self.version,
                start,
                level,
                "unterminated block comment",
            );
            if let Err(error) = result {
                self.errors.push(error);
                return;
            }
            // The delimiters (`--[=*[` and `]=*]`) never hold a line break,
            // so scanning the whole comment is equivalent to scanning only
            // the body.
            if has_line_break(self.cursor.text_since(start), self.version) {
                CommentKind::MultiLineBlock
            } else {
                CommentKind::SingleLineBlock
            }
        } else {
            self.cursor.advance_to_line_break();
            CommentKind::Line
        };

        let comment = PendingComment {
            span: self.cursor.span_since(start),
            kind,
            is_preceded_by_newline: self.has_newline_since_last_token,
            is_followed_by_newline: self.cursor.is_at_line_end(self.version),
        };
        // A comment with no newline since the previous token trails it, and
        // any other leads the next token.
        if let Some(token_start) = self.last_token_start
            && !comment.is_preceded_by_newline
        {
            self.comments.push(comment.attach(token_start, CommentPosition::Trailing));
        } else {
            self.pending_leading_comments.push(comment);
        }
        if kind == CommentKind::MultiLineBlock {
            self.has_newline_since_last_token = true;
        }
    }

    fn lex_identifier(&mut self) -> Token {
        let start = self.cursor.position();
        self.cursor.eat_while(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        let text = self.cursor.text_since(start);

        let kind = TokenKind::keyword(text, self.version)
            .unwrap_or_else(|| TokenKind::Identifier(text.into()));
        self.make_token(kind, start)
    }

    fn lex_number(&mut self) -> Option<Token> {
        let start = self.cursor.position();
        let result = scan_number(&mut self.cursor, self.version);
        let kind = self.ok_or_record(result)?;
        Some(self.make_token(kind, start))
    }

    fn lex_short_string(&mut self, quote: u8) -> Option<Token> {
        let start = self.cursor.position();
        let result = scan_short_string(&mut self.cursor, self.version, quote);
        let kind = self.ok_or_record(result)?;
        Some(self.make_token(kind, start))
    }

    fn lex_long_string_or_bracket(&mut self) -> Option<Token> {
        let start = self.cursor.position();
        let Some(level) = eat_long_bracket_open(&mut self.cursor) else {
            return self.lex_symbol();
        };
        let result = scan_long_string(&mut self.cursor, self.version, start, level);
        let kind = self.ok_or_record(result)?;
        Some(self.make_token(kind, start))
    }

    /// Lex from an opening backtick to the first `{` or the closing backtick.
    /// A string with no expressions becomes an empty `InterpolatedStringBegin`
    /// plus a queued `InterpolatedStringEnd` holding its text, so the parser
    /// always sees a pair.
    fn lex_interpolated_string(&mut self) -> Option<Token> {
        let start = self.cursor.position();
        self.cursor.advance();
        let result = scan_interpolated_segment(&mut self.cursor, self.version, start);
        let token = match self.ok_or_record(result)? {
            (text, InterpolatedSegmentEnd::OpenBrace) => {
                self.interpolation_brace_depths.push(0);
                self.make_token(TokenKind::InterpolatedStringBegin(text), start)
            }
            (text, InterpolatedSegmentEnd::Backtick) => {
                let closing_backtick = self.cursor.position().saturating_sub(1);
                let begin = self.make_token(
                    TokenKind::InterpolatedStringBegin(CompactString::default()),
                    start,
                );
                self.queued =
                    Some(self.make_token(TokenKind::InterpolatedStringEnd(text), closing_backtick));
                begin
            }
        };
        Some(token)
    }

    /// Resume an interpolated string at the `}` that closes an expression,
    /// producing `InterpolatedStringMiddle` at the next `{` or
    /// `InterpolatedStringEnd` at the closing backtick.
    fn lex_interpolated_string_continuation(&mut self) -> Option<Token> {
        self.interpolation_brace_depths.pop();
        let start = self.cursor.position();
        self.cursor.advance();
        let result = scan_interpolated_segment(&mut self.cursor, self.version, start);
        let token = match self.ok_or_record(result)? {
            (text, InterpolatedSegmentEnd::OpenBrace) => {
                self.interpolation_brace_depths.push(0);
                self.make_token(TokenKind::InterpolatedStringMiddle(text), start)
            }
            (text, InterpolatedSegmentEnd::Backtick) => {
                self.make_token(TokenKind::InterpolatedStringEnd(text), start)
            }
        };
        Some(token)
    }

    fn lex_symbol(&mut self) -> Option<Token> {
        let start = self.cursor.position();
        let byte = self.cursor.advance()?;

        let kind = match byte {
            b'+' => self.compound_or(TokenKind::PlusEqual, TokenKind::Plus),
            b'*' => self.compound_or(TokenKind::StarEqual, TokenKind::Star),
            b'%' => self.compound_or(TokenKind::PercentEqual, TokenKind::Percent),
            b'^' => self.compound_or(TokenKind::CaretEqual, TokenKind::Caret),
            b'(' => TokenKind::LeftParen,
            b')' => TokenKind::RightParen,
            b'{' => {
                if let Some(depth) = self.interpolation_brace_depths.last_mut() {
                    *depth = depth.saturating_add(1);
                }
                TokenKind::LeftBrace
            }
            b'}' => {
                if let Some(depth) = self.interpolation_brace_depths.last_mut() {
                    *depth = depth.saturating_sub(1);
                }
                TokenKind::RightBrace
            }
            b'[' => TokenKind::LeftBracket,
            b']' => TokenKind::RightBracket,
            b';' => TokenKind::Semicolon,
            b',' => TokenKind::Comma,
            b'#' => TokenKind::Hash,
            b'-' => {
                if self.version.has_type_syntax() && self.cursor.eat(b'>') {
                    TokenKind::Arrow
                } else {
                    self.compound_or(TokenKind::MinusEqual, TokenKind::Minus)
                }
            }
            b'/' => {
                if self.version.has_floor_div() && self.cursor.eat(b'/') {
                    self.compound_or(TokenKind::FloorDivEqual, TokenKind::FloorDiv)
                } else {
                    self.compound_or(TokenKind::SlashEqual, TokenKind::Slash)
                }
            }
            b'.' => self.dot_kind(),
            b':' => {
                if self.cursor.eat(b':') {
                    TokenKind::DoubleColon
                } else {
                    TokenKind::Colon
                }
            }
            b'=' => {
                if self.cursor.eat(b'=') {
                    TokenKind::EqualEqual
                } else {
                    TokenKind::Equal
                }
            }
            b'~' => {
                if self.cursor.eat(b'=') {
                    TokenKind::TildeEqual
                } else if self.version.has_bitwise_ops() {
                    TokenKind::Tilde
                } else {
                    return self.reject(
                        start,
                        "standalone `~` is not supported in this Lua version (use `~=` for not-equal)",
                    );
                }
            }
            // Luau uses `&` and `|` for intersection and union types.
            b'&' | b'|' if !self.version.has_bitwise_ops() && !self.version.has_type_syntax() => {
                return self.reject_unsupported_symbol(start, byte);
            }
            b'@' if !self.version.has_function_attributes() => {
                return self.reject_unsupported_symbol(start, byte);
            }
            b'?' if !self.version.has_type_syntax() => {
                return self.reject_unsupported_symbol(start, byte);
            }
            b'&' => TokenKind::Ampersand,
            b'|' => TokenKind::Pipe,
            b'@' => TokenKind::At,
            b'?' => TokenKind::Question,
            b'<' => {
                self.angle_kind(byte, TokenKind::LessEqual, TokenKind::ShiftLeft, TokenKind::Less)
            }
            b'>' => self.angle_kind(
                byte,
                TokenKind::GreaterEqual,
                TokenKind::ShiftRight,
                TokenKind::Greater,
            ),
            _ => return self.reject_unexpected_symbol(start, byte),
        };

        Some(self.make_token(kind, start))
    }

    /// `compound` when a compound-assignment `=` follows, consuming it, else
    /// `plain`.
    #[inline]
    fn compound_or(&mut self, compound: TokenKind, plain: TokenKind) -> TokenKind {
        if self.version.has_compound_assignment() && self.cursor.eat(b'=') {
            compound
        } else {
            plain
        }
    }

    #[inline]
    fn dot_kind(&mut self) -> TokenKind {
        if !self.cursor.eat(b'.') {
            return TokenKind::Dot;
        }
        if self.cursor.eat(b'.') {
            TokenKind::DotDotDot
        } else {
            self.compound_or(TokenKind::DotDotEqual, TokenKind::DotDot)
        }
    }

    /// Finish `<` or `>`: `or_equal` before `=`, `shift` before a second
    /// `angle` where the version has shifts, else `plain`.
    #[inline]
    fn angle_kind(
        &mut self,
        angle: u8,
        or_equal: TokenKind,
        shift: TokenKind,
        plain: TokenKind,
    ) -> TokenKind {
        if self.cursor.eat(b'=') {
            or_equal
        } else if self.version.has_bitwise_ops() && self.cursor.eat(angle) {
            shift
        } else {
            plain
        }
    }

    #[cold]
    fn reject_unsupported_symbol(&mut self, start: usize, byte: u8) -> Option<Token> {
        let symbol = char::from(byte);
        self.reject(start, format!("`{symbol}` is not supported in this Lua version"))
    }

    #[cold]
    fn reject_unexpected_symbol(&mut self, start: usize, byte: u8) -> Option<Token> {
        let unexpected = self.cursor.eat_rest_of_char(start).unwrap_or_else(|| char::from(byte));
        self.reject(start, format!("unexpected character `{unexpected}`"))
    }
}
