use crate::cursor::Cursor;
use crate::number::lex_number;
use crate::search::{ByteMatchTable, byte_match_table};
use crate::string::{lex_short_string, skip_long_bracket_open, try_count_long_bracket_level};
use crate::{LexError, LexResult};
use luck_token::{
    Comment, CommentKind, CommentPosition, CompactString, LuaVersion, Span, Token, TokenKind,
};

// Stop bytes for raw interpolated-string text: escape lead, interpolation
// opener, closing backtick, and the line breaks Luau rejects.
static INTERP_STOP: ByteMatchTable =
    byte_match_table!(|byte| matches!(byte, b'\\' | b'{' | b'`' | b'\n' | b'\r'));

/// Stateful lexer that produces tokens, comments, and errors from Lua source.
pub struct Lexer<'src> {
    cursor: Cursor<'src>,
    source: &'src str,
    version: LuaVersion,
    /// Second token of a double emit (empty interpolated strings produce
    /// InterpBegin + InterpEnd from one dispatch), handed out first on
    /// the next `next_token` call.
    queued: Option<Token>,
    comments: Vec<Comment>,
    errors: Vec<LexError>,
    last_token_start: u32,
    has_token: bool,
    saw_newline_since_last_token: bool,
    /// Leading comments waiting for their `attached_to` to be set when the next token is found.
    pending_leading_comments: Vec<PendingComment>,
    /// Stack of brace depths for nested interpolated strings (Luau).
    /// Each entry tracks how many unmatched `{` exist within the current interpolation expression.
    /// When `}` is encountered and the top of the stack is 0, we resume scanning the string.
    interp_brace_stack: Vec<u32>,
}

struct PendingComment {
    span: Span,
    kind: CommentKind,
    preceded_by_newline: bool,
    followed_by_newline: bool,
}

impl<'src> Lexer<'src> {
    pub fn new(source: &'src str, version: LuaVersion) -> Self {
        let mut lexer = Self {
            cursor: Cursor::new(source.as_bytes()),
            source,
            version,
            queued: None,
            comments: Vec::new(),
            errors: Vec::new(),
            last_token_start: 0,
            has_token: false,
            saw_newline_since_last_token: true, // start of file counts as "after newline"
            pending_leading_comments: Vec::new(),
            interp_brace_stack: Vec::new(),
        };

        // UTF-8 BOM: PUC Lua's loadfile and Luau both skip it.
        if lexer.source.starts_with('\u{FEFF}') {
            lexer.cursor.advance();
            lexer.cursor.advance();
            lexer.cursor.advance();
        }

        // Lua skips any first line beginning with '#', not just '#!'.
        if lexer.cursor.peek() == Some(b'#') {
            lexer.lex_shebang();
        }

        lexer
    }

    pub fn tokenize(&mut self) -> LexResult {
        let mut tokens = Vec::new();
        loop {
            self.handle_whitespace();
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

        LexResult {
            tokens,
            comments: std::mem::take(&mut self.comments),
            errors: std::mem::take(&mut self.errors),
        }
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
            self.handle_whitespace();
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
    pub fn finish(self) -> (Vec<Comment>, Vec<LexError>) {
        (self.comments, self.errors)
    }

    /// Single-jump dispatch on the lead byte; a match compiles to the
    /// same jump table as a fn-pointer array but
    /// keeps the small handlers inlinable. Whitespace never reaches here -
    /// the tokenize loop consumes it before dispatching. Returns `None`
    /// when the dispatch produced no token (comments, error recovery).
    #[inline]
    fn dispatch_byte(&mut self, byte: u8) -> Option<Token> {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => Some(self.lex_identifier()),
            b'0'..=b'9' => self.handle_digit(),
            b'"' | b'\'' => self.handle_quote(),
            b'`' => self.handle_backtick(),
            b'-' => self.handle_minus(),
            b'[' => self.handle_bracket_open(),
            b'.' => self.handle_dot(),
            _ => self.lex_symbol(),
        }
    }

    fn handle_whitespace(&mut self) {
        while let Some(byte) = self.cursor.peek() {
            match byte {
                b' ' | b'\t' | 0x0B | 0x0C => {
                    self.cursor.advance();
                }
                b'\n' => {
                    self.cursor.advance();
                    self.saw_newline_since_last_token = true;
                }
                b'\r' => {
                    self.cursor.advance();
                    if self.cursor.peek() == Some(b'\n') {
                        self.cursor.advance();
                    }
                    self.saw_newline_since_last_token = true;
                }
                _ => break,
            }
        }
    }

    fn handle_digit(&mut self) -> Option<Token> {
        let start = self.cursor.position();
        match lex_number(&mut self.cursor, self.source, self.version) {
            Ok(kind) => Some(self.make_token(kind, start)),
            Err(err) => {
                self.errors.push(err);
                None
            }
        }
    }

    fn handle_dot(&mut self) -> Option<Token> {
        if self.cursor.peek_at(1).is_some_and(|b| b.is_ascii_digit()) {
            self.handle_digit()
        } else {
            self.lex_symbol()
        }
    }

    fn handle_quote(&mut self) -> Option<Token> {
        let start = self.cursor.position();
        match lex_short_string(&mut self.cursor, self.source, self.version) {
            Ok(kind) => Some(self.make_token(kind, start)),
            Err(err) => {
                self.errors.push(err);
                None
            }
        }
    }

    fn handle_backtick(&mut self) -> Option<Token> {
        if self.version.is_luau() {
            self.lex_interpolated_string()
        } else {
            self.lex_symbol()
        }
    }

    fn handle_minus(&mut self) -> Option<Token> {
        if self.cursor.peek_at(1) == Some(b'-') {
            self.lex_comment();
            None
        } else {
            self.lex_symbol()
        }
    }

    fn handle_bracket_open(&mut self) -> Option<Token> {
        if let Some(level) = try_count_long_bracket_level(&self.cursor) {
            let start = self.cursor.position();
            skip_long_bracket_open(&mut self.cursor, level);
            match crate::string::lex_long_bracket_body(&mut self.cursor, self.source, start, level)
            {
                Ok(Some(kind)) => Some(self.make_token(kind, start)),
                Ok(None) => unreachable!("level was already validated"),
                Err(err) => {
                    self.errors.push(err);
                    None
                }
            }
        } else {
            self.lex_symbol()
        }
    }

    #[inline]
    fn make_token(&mut self, kind: TokenKind, start: usize) -> Token {
        let span = Span::new(start as u32, self.cursor.position() as u32);

        for pending in self.pending_leading_comments.drain(..) {
            self.comments.push(Comment {
                span: pending.span,
                attached_to: span.start,
                kind: pending.kind,
                position: CommentPosition::Leading,
                preceded_by_newline: pending.preceded_by_newline,
                followed_by_newline: pending.followed_by_newline,
            });
        }

        self.last_token_start = span.start;
        self.has_token = true;
        self.saw_newline_since_last_token = false;
        Token::new(kind, span)
    }

    fn lex_shebang(&mut self) {
        let start = self.cursor.position();
        let rest = self.cursor.rest();
        let line_len = memchr::memchr2(b'\n', b'\r', rest).unwrap_or(rest.len());
        self.cursor.advance_by(line_len);
        let span = Span::new(start as u32, self.cursor.position() as u32);

        let followed_by_newline = matches!(self.cursor.peek(), Some(b'\n') | Some(b'\r') | None);

        self.pending_leading_comments.push(PendingComment {
            span,
            kind: CommentKind::Shebang,
            preceded_by_newline: false,
            followed_by_newline,
        });
    }

    fn lex_comment(&mut self) {
        let start = self.cursor.position();
        let preceded_by_newline = self.saw_newline_since_last_token;

        self.cursor.advance(); // -
        self.cursor.advance(); // -
        if let Some(level) = try_count_long_bracket_level(&self.cursor) {
            skip_long_bracket_open(&mut self.cursor, level);
            match self.lex_block_comment_body(start, level) {
                Ok(()) => {}
                Err(err) => {
                    self.errors.push(err);
                }
            }
            return;
        }

        let rest = self.cursor.rest();
        let line_len = memchr::memchr2(b'\n', b'\r', rest).unwrap_or(rest.len());
        self.cursor.advance_by(line_len);

        let span = Span::new(start as u32, self.cursor.position() as u32);
        let followed_by_newline = matches!(self.cursor.peek(), Some(b'\n') | Some(b'\r') | None);

        if preceded_by_newline || !self.has_token {
            self.pending_leading_comments.push(PendingComment {
                span,
                kind: CommentKind::Line,
                preceded_by_newline,
                followed_by_newline,
            });
        } else {
            self.comments.push(Comment {
                span,
                attached_to: self.last_token_start,
                kind: CommentKind::Line,
                position: CommentPosition::Trailing,
                preceded_by_newline,
                followed_by_newline,
            });
        }
    }

    fn lex_block_comment_body(&mut self, start: usize, level: usize) -> Result<(), LexError> {
        let preceded_by_newline = self.saw_newline_since_last_token;
        let mut has_newline_in_body = false;

        loop {
            let rest = self.cursor.rest();
            let Some(bracket_offset) = memchr::memchr(b']', rest) else {
                self.cursor.advance_by(rest.len());
                return Err(crate::lex_error(
                    Span::new(start as u32, self.cursor.position() as u32),
                    "unterminated block comment",
                ));
            };
            if !has_newline_in_body
                && memchr::memchr2(b'\n', b'\r', &rest[..bracket_offset]).is_some()
            {
                has_newline_in_body = true;
            }
            self.cursor.advance_by(bracket_offset);
            let mut closing_level = 0;
            let mut offset = 1;
            while self.cursor.peek_at(offset) == Some(b'=') {
                closing_level += 1;
                offset += 1;
            }
            if closing_level == level && self.cursor.peek_at(offset) == Some(b']') {
                self.cursor.advance_by(offset + 1);
                break;
            }
            self.cursor.advance();
        }

        let span = Span::new(start as u32, self.cursor.position() as u32);
        let kind = if has_newline_in_body {
            CommentKind::MultiLineBlock
        } else {
            CommentKind::SingleLineBlock
        };

        let mut followed_by_newline = false;
        let mut temp_offset = 0;
        loop {
            match self.cursor.peek_at(temp_offset) {
                Some(b' ') | Some(b'\t') => temp_offset += 1,
                Some(b'\n') | Some(b'\r') | None => {
                    followed_by_newline = true;
                    break;
                }
                _ => break,
            }
        }

        if preceded_by_newline || !self.has_token {
            self.pending_leading_comments.push(PendingComment {
                span,
                kind,
                preceded_by_newline,
                followed_by_newline,
            });
        } else {
            self.comments.push(Comment {
                span,
                attached_to: self.last_token_start,
                kind,
                position: CommentPosition::Trailing,
                preceded_by_newline,
                followed_by_newline,
            });
        }

        Ok(())
    }

    fn lex_identifier(&mut self) -> Token {
        let start = self.cursor.position();
        self.cursor
            .eat_while(|b| b.is_ascii_alphanumeric() || b == b'_');
        let text = &self.source[start..self.cursor.position()];

        let kind =
            match_keyword(text, self.version).unwrap_or_else(|| TokenKind::Identifier(text.into()));
        self.make_token(kind, start)
    }

    fn lex_symbol(&mut self) -> Option<Token> {
        let start = self.cursor.position();
        let byte = self
            .cursor
            .advance()
            .expect("called after EOF check in tokenize loop");

        let kind = match byte {
            b'+' => {
                if self.version.is_luau() && self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::PlusEqual
                } else {
                    TokenKind::Plus
                }
            }
            b'*' => {
                if self.version.is_luau() && self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::StarEqual
                } else {
                    TokenKind::Star
                }
            }
            b'%' => {
                if self.version.is_luau() && self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::PercentEqual
                } else {
                    TokenKind::Percent
                }
            }
            b'^' => {
                if self.version.is_luau() && self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::CaretEqual
                } else {
                    TokenKind::Caret
                }
            }
            b'(' => TokenKind::LeftParen,
            b')' => TokenKind::RightParen,
            b'{' => {
                if let Some(depth) = self.interp_brace_stack.last_mut() {
                    *depth += 1;
                }
                TokenKind::LeftBrace
            }
            b'}' => {
                if let Some(depth) = self.interp_brace_stack.last_mut() {
                    if *depth == 0 {
                        // Depth 0 means this `}` closes the interpolation - resume string scanning
                        self.interp_brace_stack.pop();
                        return self.lex_interp_continuation(start);
                    } else {
                        *depth -= 1;
                    }
                }
                TokenKind::RightBrace
            }
            b'[' => TokenKind::LeftBracket,
            b']' => TokenKind::RightBracket,
            b';' => TokenKind::Semicolon,
            b',' => TokenKind::Comma,
            b'#' => TokenKind::Hash,
            b'-' => {
                if self.version.is_luau() && self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::MinusEqual
                } else if self.version.is_luau() && self.cursor.peek() == Some(b'>') {
                    self.cursor.advance();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            b'/' => {
                if self.cursor.peek() == Some(b'/') {
                    if self.version.has_floor_div() {
                        self.cursor.advance();
                        if self.version.is_luau() && self.cursor.peek() == Some(b'=') {
                            self.cursor.advance();
                            TokenKind::FloorDivEqual
                        } else {
                            TokenKind::FloorDiv
                        }
                    } else {
                        TokenKind::Slash
                    }
                } else if self.version.is_luau() && self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::SlashEqual
                } else {
                    TokenKind::Slash
                }
            }
            b'.' => {
                if self.cursor.peek() == Some(b'.') {
                    self.cursor.advance();
                    if self.cursor.peek() == Some(b'.') {
                        self.cursor.advance();
                        TokenKind::DotDotDot
                    } else if self.version.is_luau() && self.cursor.peek() == Some(b'=') {
                        self.cursor.advance();
                        TokenKind::DotDotEqual
                    } else {
                        TokenKind::DotDot
                    }
                } else {
                    TokenKind::Dot
                }
            }
            b':' => {
                if self.cursor.peek() == Some(b':') {
                    self.cursor.advance();
                    TokenKind::DoubleColon
                } else {
                    TokenKind::Colon
                }
            }
            b'=' => {
                if self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::EqualEqual
                } else {
                    TokenKind::Equal
                }
            }
            b'~' => {
                if self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::TildeEqual
                } else if self.version.has_bitwise_ops() {
                    TokenKind::Tilde
                } else {
                    self.errors.push(crate::lex_error(Span::new(start as u32, self.cursor.position() as u32), "standalone '~' is not supported in this Lua version (use '~=' for not-equal)"));
                    return None;
                }
            }
            b'&' => {
                // Luau uses `&` for intersection types
                if self.version.has_bitwise_ops() || self.version.is_luau() {
                    TokenKind::Ampersand
                } else {
                    self.errors.push(crate::lex_error(
                        Span::new(start as u32, self.cursor.position() as u32),
                        "'&' is not supported in this Lua version",
                    ));
                    return None;
                }
            }
            b'|' => {
                // Luau uses `|` for union types
                if self.version.has_bitwise_ops() || self.version.is_luau() {
                    TokenKind::Pipe
                } else {
                    self.errors.push(crate::lex_error(
                        Span::new(start as u32, self.cursor.position() as u32),
                        "'|' is not supported in this Lua version",
                    ));
                    return None;
                }
            }
            b'<' => {
                if self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::LessEqual
                } else if self.cursor.peek() == Some(b'<') && self.version.has_bitwise_ops() {
                    self.cursor.advance();
                    TokenKind::ShiftLeft
                } else {
                    TokenKind::Less
                }
            }
            b'>' => {
                if self.cursor.peek() == Some(b'=') {
                    self.cursor.advance();
                    TokenKind::GreaterEqual
                } else if self.cursor.peek() == Some(b'>') && self.version.has_bitwise_ops() {
                    self.cursor.advance();
                    TokenKind::ShiftRight
                } else {
                    TokenKind::Greater
                }
            }
            b'@' => {
                if self.version.is_luau() {
                    TokenKind::At
                } else {
                    self.errors.push(crate::lex_error(
                        Span::new(start as u32, self.cursor.position() as u32),
                        "'@' is not supported in this Lua version",
                    ));
                    return None;
                }
            }
            b'?' => {
                if self.version.is_luau() {
                    TokenKind::Question
                } else {
                    self.errors.push(crate::lex_error(
                        Span::new(start as u32, self.cursor.position() as u32),
                        "'?' is not supported in this Lua version",
                    ));
                    return None;
                }
            }
            _ => {
                // Consume the full UTF-8 sequence so a multi-byte char
                // yields one error, not one mojibake error per byte.
                while self.cursor.peek().is_some_and(|next| (next & 0xC0) == 0x80) {
                    self.cursor.advance();
                }
                let unexpected = self.source[start..].chars().next().unwrap_or(byte as char);
                self.errors.push(crate::lex_error(
                    Span::new(start as u32, self.cursor.position() as u32),
                    format!("unexpected character '{unexpected}'"),
                ));
                return None;
            }
        };

        Some(self.make_token(kind, start))
    }

    /// Resume scanning an interpolated string after `}` closes an expression.
    /// Produces InterpMid (if another `{` follows) or InterpEnd (if `` ` `` closes the string).
    fn lex_interp_continuation(&mut self, start: usize) -> Option<Token> {
        match self.scan_interp_segment(start) {
            None => None,
            Some((text, InterpSegmentEnd::OpenBrace)) => {
                let token = self.make_token(TokenKind::InterpMid(text), start);
                self.interp_brace_stack.push(0);
                Some(token)
            }
            Some((text, InterpSegmentEnd::Backtick)) => {
                Some(self.make_token(TokenKind::InterpEnd(text), start))
            }
        }
    }

    /// Lex an interpolated string starting at backtick. Produces InterpBegin + expression
    /// tokens + InterpMid/InterpEnd sequences. Plain strings emit InterpBegin("") + InterpEnd(text).
    fn lex_interpolated_string(&mut self) -> Option<Token> {
        let start = self.cursor.position();
        self.cursor.advance();

        match self.scan_interp_segment(start) {
            None => None,
            Some((text, InterpSegmentEnd::OpenBrace)) => {
                let token = self.make_token(TokenKind::InterpBegin(text), start);
                self.interp_brace_stack.push(0);
                Some(token)
            }
            Some((text, InterpSegmentEnd::Backtick)) => {
                // Emit InterpBegin("") + InterpEnd(text) so parser sees a consistent begin/end pair
                let end_pos = self.cursor.position();
                let begin =
                    self.make_token(TokenKind::InterpBegin(CompactString::default()), start);
                self.queued = Some(self.make_token(TokenKind::InterpEnd(text), end_pos - 1));
                Some(begin)
            }
        }
    }

    /// Scan raw interpolated-string text up to an interpolation opener `{`
    /// or the closing backtick. Text is accumulated as source slices, never
    /// byte-by-byte, so multi-byte UTF-8 stays intact. Returns `None` after
    /// pushing an error.
    fn scan_interp_segment(&mut self, start: usize) -> Option<(CompactString, InterpSegmentEnd)> {
        let mut text = CompactString::default();
        let mut segment_start = self.cursor.position();
        loop {
            match self.cursor.peek() {
                None => {
                    self.errors.push(crate::lex_error(
                        Span::new(start as u32, self.cursor.position() as u32),
                        "unterminated interpolated string",
                    ));
                    return None;
                }
                // Luau rejects unescaped line breaks in backtick strings,
                // same as in short strings.
                Some(b'\n' | b'\r') => {
                    self.errors.push(crate::lex_error(
                        Span::new(start as u32, self.cursor.position() as u32),
                        "unterminated interpolated string",
                    ));
                    return None;
                }
                Some(b'\\') => {
                    text.push_str(&self.source[segment_start..self.cursor.position()]);
                    text.push('\\');
                    self.cursor.advance();
                    // `\u{...}` is a unicode escape; without this the `{`
                    // would be mis-lexed as an interpolation opener.
                    if self.cursor.peek() == Some(b'u') && self.cursor.peek_at(1) == Some(b'{') {
                        text.push('u');
                        self.cursor.advance();
                        text.push('{');
                        self.cursor.advance();
                        loop {
                            match self.cursor.peek() {
                                Some(b'}') => {
                                    text.push('}');
                                    self.cursor.advance();
                                    break;
                                }
                                Some(digit) if digit.is_ascii_hexdigit() => {
                                    text.push(digit as char);
                                    self.cursor.advance();
                                }
                                _ => {
                                    self.errors.push(crate::lex_error(
                                        Span::new(start as u32, self.cursor.position() as u32),
                                        "malformed \\u{...} escape in interpolated string",
                                    ));
                                    return None;
                                }
                            }
                        }
                    } else if self.cursor.peek().is_some() {
                        // Copy the escaped character whole (it may be
                        // multi-byte); continuation bytes are 0b10xxxxxx.
                        let escaped_start = self.cursor.position();
                        self.cursor.advance();
                        while self.cursor.peek().is_some_and(|byte| (byte & 0xC0) == 0x80) {
                            self.cursor.advance();
                        }
                        // `\z` skips following whitespace, including line
                        // breaks; keep the raw run so payloads re-emit as-is.
                        if self.source.as_bytes()[escaped_start] == b'z' {
                            while self
                                .cursor
                                .peek()
                                .is_some_and(|byte| byte.is_ascii_whitespace())
                            {
                                self.cursor.advance();
                            }
                        }
                        text.push_str(&self.source[escaped_start..self.cursor.position()]);
                    }
                    segment_start = self.cursor.position();
                }
                Some(b'{') => {
                    text.push_str(&self.source[segment_start..self.cursor.position()]);
                    self.cursor.advance();
                    if self.cursor.peek() == Some(b'{') {
                        self.errors.push(crate::lex_error(Span::new(
                                (self.cursor.position() - 1) as u32,
                                (self.cursor.position() + 1) as u32,
                            ), "'{{' is not allowed in interpolated strings; use '\\{' for a literal brace"));
                        return None;
                    }
                    return Some((text, InterpSegmentEnd::OpenBrace));
                }
                Some(b'`') => {
                    text.push_str(&self.source[segment_start..self.cursor.position()]);
                    self.cursor.advance();
                    return Some((text, InterpSegmentEnd::Backtick));
                }
                Some(_) => {
                    self.cursor.advance();
                    self.cursor.advance_until_match(&INTERP_STOP);
                }
            }
        }
    }
}

/// Keyword lookup behind a cheap pre-gate: every Lua keyword is 2-8
/// bytes of lowercase ASCII, so most identifiers bail on two compares and LLVM
/// compiles the match into a length-first switch.
#[inline]
fn match_keyword(text: &str, version: LuaVersion) -> Option<TokenKind> {
    if text.len() < 2 || text.len() > 8 || !text.as_bytes()[0].is_ascii_lowercase() {
        return None;
    }
    let kind = match text {
        "and" => TokenKind::And,
        "break" => TokenKind::Break,
        "do" => TokenKind::Do,
        "else" => TokenKind::Else,
        "elseif" => TokenKind::ElseIf,
        "end" => TokenKind::End,
        "false" => TokenKind::False,
        "for" => TokenKind::For,
        "function" => TokenKind::Function,
        "if" => TokenKind::If,
        "in" => TokenKind::In,
        "local" => TokenKind::Local,
        "nil" => TokenKind::Nil,
        "not" => TokenKind::Not,
        "or" => TokenKind::Or,
        "repeat" => TokenKind::Repeat,
        "return" => TokenKind::Return,
        "then" => TokenKind::Then,
        "true" => TokenKind::True,
        "until" => TokenKind::Until,
        "while" => TokenKind::While,
        // Lua 5.2+
        "goto" if version.has_goto() => TokenKind::Goto,
        // Lua 5.5
        "global" if version.has_global() => TokenKind::Global,
        _ => return None,
    };
    Some(kind)
}

/// How a raw interpolated-string segment ended.
enum InterpSegmentEnd {
    /// `{` - an interpolation expression follows.
    OpenBrace,
    /// `` ` `` - the string is complete.
    Backtick,
}
