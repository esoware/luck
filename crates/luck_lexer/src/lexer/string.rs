use luck_diagnostics::Diagnostic;
use luck_token::{EscapeError, LuaVersion, Span, Token, TokenKind, read_escape};
use memchr::memmem;

use super::Lexer;
use crate::cursor::Cursor;
use crate::search::{ByteMatchTable, byte_match_table, find_match};

// Stop bytes for raw interpolated-string text: escape lead, interpolation
// opener, closing backtick, and the line breaks Luau rejects.
static INTERPOLATED_STRING_STOP: ByteMatchTable =
    byte_match_table!(|b| matches!(b, b'\\' | b'{' | b'`' | b'\n' | b'\r'));

#[derive(Clone, Copy, PartialEq, Eq)]
enum SegmentEnd {
    OpenBrace,
    Backtick,
    /// The line or the source ended before the closing backtick.
    Unterminated,
}

impl Lexer<'_> {
    /// Lex a single- or double-quoted string that opened with `quote` at
    /// `start`, with the cursor past it.
    ///
    /// A malformed escape goes to `errors` and stays text, so the string ends
    /// where it would have without it. A line break or the end of the source
    /// before the closing quote ends the string there, unterminated, so what
    /// follows on later lines lexes as it would have.
    pub(super) fn lex_short_string(&mut self, start: usize, quote: u8) -> Token {
        loop {
            self.cursor.eat_short_string_run(quote);
            match self.cursor.peek() {
                None | Some(b'\n' | b'\r') => {
                    self.reject(start, "unterminated string");
                    return self.unterminated_token(TokenKind::StringLiteral, start);
                }
                Some(b'\\') => {
                    self.cursor.bump();
                    // The next iteration reports the unterminated string.
                    if self.cursor.peek().is_some() {
                        self.scan_escape(false);
                    }
                }
                // The scan stops only at those bytes and at `quote`.
                Some(_) => {
                    self.cursor.bump();
                    return self.token(TokenKind::StringLiteral, start);
                }
            }
        }
    }

    /// Lex a long-bracket string of `level` that opened at `start`, with the
    /// cursor past its `[=*[` opener. A missing closer runs the string to the
    /// end of the source.
    pub(super) fn lex_long_string(&mut self, start: usize, level: usize) -> Token {
        if advance_past_long_bracket_close(
            &mut self.cursor,
            self.version,
            start,
            level,
            "unterminated long bracket string",
            &mut self.errors,
        ) {
            self.token(TokenKind::StringLiteral, start)
        } else {
            self.unterminated_token(TokenKind::StringLiteral, start)
        }
    }

    /// Lex an interpolated string's first piece, with the cursor past its
    /// opening backtick: `InterpolatedStringBegin` up to an interpolation's
    /// `{`, or the whole of a string with none as `InterpolatedStringSimple`.
    pub(super) fn lex_interpolated_string(&mut self, start: usize) -> Token {
        match self.scan_interpolated_segment(start) {
            SegmentEnd::OpenBrace => {
                self.interpolation_brace_depths.push(0);
                self.token(TokenKind::InterpolatedStringBegin, start)
            }
            SegmentEnd::Backtick => self.token(TokenKind::InterpolatedStringSimple, start),
            SegmentEnd::Unterminated => {
                self.unterminated_token(TokenKind::InterpolatedStringSimple, start)
            }
        }
    }

    /// Resume an interpolated string at the `}` that closes an expression,
    /// with the cursor past it, producing `InterpolatedStringMiddle` up to the
    /// next `{` or `InterpolatedStringEnd` through the closing backtick.
    ///
    /// An unterminated string ends at its line break, so the `}` always
    /// yields one, and the lexer never closes a string the parser doesn't see
    /// close.
    pub(super) fn lex_interpolated_string_continuation(&mut self, start: usize) -> Token {
        self.interpolation_brace_depths.pop();
        match self.scan_interpolated_segment(start) {
            SegmentEnd::OpenBrace => {
                self.interpolation_brace_depths.push(0);
                self.token(TokenKind::InterpolatedStringMiddle, start)
            }
            SegmentEnd::Backtick => self.token(TokenKind::InterpolatedStringEnd, start),
            SegmentEnd::Unterminated => {
                self.unterminated_token(TokenKind::InterpolatedStringEnd, start)
            }
        }
    }

    /// The token of a string from `start` that ends at the cursor without its
    /// closing delimiter, whose span also goes to `unterminated`.
    #[cold]
    #[inline(never)]
    fn unterminated_token(&mut self, kind: TokenKind, start: usize) -> Token {
        let mut token = self.token(kind, start);
        token.is_unterminated = true;
        self.unterminated.push(token.span);
        token
    }

    /// Consume one escape sequence, with the cursor on the byte after `\`,
    /// as [`read_escape`] reads it for every stage. A malformed one goes to
    /// `errors`, and one only Lua 5.1 and Luau read, as the character it
    /// escapes, to `lenient_escapes`.
    fn scan_escape(&mut self, is_interpolated: bool) {
        let escape_start = self.cursor.position().saturating_sub(1);
        let (length, meaning) = read_escape(self.cursor.rest(), is_interpolated, self.version);
        match meaning {
            Ok(escaped) => {
                let character_start = self.cursor.position();
                self.cursor.advance_by(length);
                if escaped.is_lenient() {
                    // The escape takes the first byte of a multi-byte
                    // character, and its span takes the whole character.
                    self.cursor.eat_rest_of_char(character_start);
                    self.lenient_escapes.push(self.cursor.span_since(escape_start));
                }
            }
            Err(EscapeError::Undefined) => {
                let character_start = self.cursor.position();
                let byte = self.cursor.peek().unwrap_or_default();
                self.cursor.bump();
                let message = self.cursor.eat_rest_of_char(character_start).map_or_else(
                    || format!("invalid escape sequence, `\\` before byte 0x{byte:02X}"),
                    |character| format!("invalid escape sequence `\\{character}`"),
                );
                self.reject(escape_start, message);
            }
            Err(error) => {
                self.cursor.advance_by(length);
                self.reject(escape_start, escape_message(error, self.version));
            }
        }
    }

    /// Scan one piece of raw interpolated-string text that began at `start`,
    /// at its backtick or the `}` that resumed the string.
    ///
    /// The piece ends at an interpolation opener `{`, the closing backtick, or
    /// the line break that leaves it unterminated, consuming the `{` or the
    /// backtick. A malformed escape or a `{{` goes to `errors` and stays text,
    /// so a piece ends where it would have without it.
    fn scan_interpolated_segment(&mut self, start: usize) -> SegmentEnd {
        loop {
            match self.cursor.peek() {
                // Luau rejects unescaped line breaks in backtick strings, same
                // as in short strings.
                None | Some(b'\n' | b'\r') => {
                    self.reject(start, "unterminated interpolated string");
                    return SegmentEnd::Unterminated;
                }
                Some(b'\\') => {
                    self.cursor.bump();
                    // Consuming `\u{...}` whole also keeps its `{` from
                    // opening an interpolation, and the next iteration
                    // reports an unterminated string.
                    if self.cursor.peek().is_some() {
                        self.scan_escape(true);
                    }
                }
                Some(b'{') if self.cursor.peek_next() == Some(b'{') => {
                    let position = self.cursor.position();
                    self.cursor.advance_by(2);
                    self.reject(
                        position,
                        "`{{` is not allowed in interpolated strings, use `\\{` for a literal brace",
                    );
                }
                Some(b'{') => {
                    self.cursor.bump();
                    return SegmentEnd::OpenBrace;
                }
                Some(b'`') => {
                    self.cursor.bump();
                    return SegmentEnd::Backtick;
                }
                Some(_) => {
                    self.cursor.bump();
                    self.cursor
                        .advance_by(find_match(self.cursor.rest(), &INTERPOLATED_STRING_STOP));
                }
            }
        }
    }
}

fn escape_message(error: EscapeError, version: LuaVersion) -> String {
    match error {
        EscapeError::DecimalTooLarge(value) => {
            format!("decimal escape too large (`\\{value}`), maximum is `\\255`")
        }
        EscapeError::HexDigits(count) => {
            format!("`\\x` escape requires exactly 2 hex digits, got {count}")
        }
        EscapeError::UnicodeBrace => "`\\u` escape requires `{`".to_owned(),
        EscapeError::UnicodeDigits => "`\\u{}` requires at least one hex digit".to_owned(),
        EscapeError::UnicodeTooLarge => {
            let limit = if version.has_extended_unicode_escape() { "2^31" } else { "0x110000" };
            format!("`\\u` codepoint too large (must be less than {limit})")
        }
        EscapeError::UnicodeUnclosed => "`\\u` escape missing closing `}`".to_owned(),
        EscapeError::Undefined => "invalid escape sequence".to_owned(),
    }
}

/// Consume the rest of a long-bracket opener `=*[` after its first `[`, which
/// the cursor is past, and return its level (`[==[` is level 2). Consumes
/// nothing and returns `None` when no opener follows.
pub(super) fn eat_long_bracket_level(cursor: &mut Cursor<'_>) -> Option<usize> {
    let rest = cursor.rest();
    let level = rest.iter().take_while(|&&b| b == b'=').count();
    if rest.get(level) != Some(&b'[') {
        return None;
    }
    cursor.advance_by(level.saturating_add(1));
    Some(level)
}

/// Advance from just past a long-bracket opener to past its matching `]=*]`
/// closer, and return whether the closer came.
///
/// Long-bracket strings and block comments share it, and name themselves in
/// `unterminated`.
///
/// A missing closer leaves the cursor at the end of the source and goes to
/// `errors`, spanning from `start`. A level-0 body holding a `[[` that
/// `version` rejects goes there too, at its first `[[`, where Lua 5.1 stops.
pub(super) fn advance_past_long_bracket_close(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
    level: usize,
    unterminated: &'static str,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let body_start = cursor.position();
    loop {
        let rest = cursor.rest();
        let Some(bracket_offset) = memchr::memchr(b']', rest) else {
            cursor.advance_by(rest.len());
            errors.push(Diagnostic::e008(cursor.span_since(start), unterminated));
            return false;
        };
        cursor.advance_by(bracket_offset.saturating_add(1));
        let after_bracket = cursor.rest();
        let closing_level = after_bracket.iter().take_while(|&&b| b == b'=').count();
        if closing_level == level && after_bracket.get(level) == Some(&b']') {
            cursor.advance_by(level.saturating_add(1));
            if level == 0
                && version.has_long_bracket_nesting_error()
                && let Some(offset) = memmem::find(cursor.text_since(body_start), b"[[")
            {
                let nested_open = body_start.saturating_add(offset);
                errors.push(Diagnostic::e008(
                    Span::from_indices(nested_open, nested_open.saturating_add(2)),
                    "nesting of `[[...]]` is deprecated",
                ));
            }
            return true;
        }
    }
}
