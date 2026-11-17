use luck_token::{LuaVersion, Span, Token, TokenKind, is_lua_whitespace, unicode_escape_codepoint};
use memchr::memmem;

use super::Lexer;
use crate::cursor::Cursor;
use crate::lex_error::{LexError, lex_error};
use crate::search::{ByteMatchTable, byte_match_table, find_match};

// Stop bytes for the short-string body scan: either quote form (the
// non-delimiter quote falls through as ordinary content), escapes, and
// the line breaks that make a string unterminated.
static SHORT_STRING_STOP: ByteMatchTable =
    byte_match_table!(|b| matches!(b, b'"' | b'\'' | b'\\' | b'\n' | b'\r'));

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
            match self.cursor.peek() {
                None | Some(b'\n' | b'\r') => {
                    self.reject(start, "unterminated string");
                    return self.unterminated_token(TokenKind::StringLiteral, start);
                }
                Some(b'\\') => {
                    self.cursor.advance_by(1);
                    // The next iteration reports the unterminated string.
                    if let Some(escaped) = self.cursor.peek()
                        && let Err(error) = scan_escape(&mut self.cursor, self.version, escaped)
                    {
                        self.errors.push(error);
                    }
                }
                Some(c) if c == quote => {
                    self.cursor.advance_by(1);
                    return self.token(TokenKind::StringLiteral, start);
                }
                Some(_) => {
                    self.cursor.advance_by(1);
                    // Most strings end within 8 bytes, so the table checks those
                    // before `memchr` takes over.
                    let bytes = self.cursor.rest();
                    let (prefix, rest) = bytes.split_at(bytes.len().min(8));
                    let prefix_stop = find_match(prefix, &SHORT_STRING_STOP);
                    self.cursor.advance_by(prefix_stop);
                    if prefix_stop == prefix.len() {
                        // Stopping at LF bounds an unterminated string to its
                        // line; the second scan finds any CR before that stop.
                        let end = memchr::memchr3(quote, b'\\', b'\n', rest).unwrap_or(rest.len());
                        let (line, _) = rest.split_at(end);
                        self.cursor.advance_by(memchr::memchr(b'\r', line).unwrap_or(end));
                    }
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
                    self.cursor.advance_by(1);
                    match self.cursor.peek() {
                        // Escaped braces and backticks stay literal text.
                        Some(b'{' | b'}' | b'`') => self.cursor.advance_by(1),
                        // Consuming `\u{...}` whole also keeps its `{` from
                        // opening an interpolation.
                        Some(escaped) => {
                            if let Err(error) = scan_escape(&mut self.cursor, self.version, escaped)
                            {
                                self.errors.push(error);
                            }
                        }
                        // The next iteration reports the unterminated string.
                        None => {}
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
                    self.cursor.advance_by(1);
                    return SegmentEnd::OpenBrace;
                }
                Some(b'`') => {
                    self.cursor.advance_by(1);
                    return SegmentEnd::Backtick;
                }
                Some(_) => {
                    self.cursor.advance_by(1);
                    self.cursor
                        .advance_by(find_match(self.cursor.rest(), &INTERPOLATED_STRING_STOP));
                }
            }
        }
    }
}

/// Consume one escape sequence, with the cursor on the byte after `\`,
/// rejecting the forms `version` doesn't define. Shared by short strings and
/// Luau interpolated strings.
///
/// # Errors
///
/// Returns a [`LexError`] for a malformed or undefined escape.
fn scan_escape(cursor: &mut Cursor<'_>, version: LuaVersion, escaped: u8) -> Result<(), LexError> {
    match escaped {
        b'\n' => {
            cursor.advance_by(1);
            if version.has_lfcr_line_break() {
                cursor.eat(b'\r');
            }
        }
        b'\r' => {
            cursor.advance_by(1);
            cursor.eat(b'\n');
        }
        c if c.is_ascii_digit() => scan_decimal_escape(cursor)?,
        b'x' if version.has_hex_escape() => scan_hex_escape(cursor)?,
        b'z' if version.has_whitespace_escape() => {
            cursor.advance_by(1);
            cursor.eat_while(is_lua_whitespace);
        }
        b'u' if version.has_unicode_escape() => scan_unicode_escape(cursor, version)?,
        b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'\'' | b'"' => {
            cursor.advance_by(1);
        }
        // Lua 5.1 and Luau accept any other escaped character as that
        // literal character; 5.2+ reject it.
        _ if !version.has_strict_escapes() => cursor.advance_by(1),
        c => return Err(invalid_escape(cursor, c)),
    }
    Ok(())
}

/// Consume `\ddd`, with the cursor on the first digit.
///
/// # Errors
///
/// Returns a [`LexError`] when the value doesn't fit a byte.
fn scan_decimal_escape(cursor: &mut Cursor<'_>) -> Result<(), LexError> {
    let digits_start = cursor.position();
    let digit_count = cursor.rest().iter().take(3).take_while(|b| b.is_ascii_digit()).count();
    cursor.advance_by(digit_count);
    let value = cursor.text_since(digits_start).parse::<u16>().unwrap_or_default();
    if value > 255 {
        return Err(lex_error(
            cursor.span_since(digits_start.saturating_sub(1)),
            format!("decimal escape too large (`\\{value}`), maximum is `\\255`"),
        ));
    }
    Ok(())
}

/// Consume `\xXX`, with the cursor on the `x`.
///
/// # Errors
///
/// Returns a [`LexError`] unless exactly two hex digits follow.
fn scan_hex_escape(cursor: &mut Cursor<'_>) -> Result<(), LexError> {
    let escape_start = cursor.position().saturating_sub(1);
    cursor.advance_by(1);
    for digit_count in 0..2u32 {
        if !cursor.peek().is_some_and(|digit| digit.is_ascii_hexdigit()) {
            return Err(lex_error(
                cursor.span_since(escape_start),
                format!("`\\x` escape requires exactly 2 hex digits, got {digit_count}"),
            ));
        }
        cursor.advance_by(1);
    }
    Ok(())
}

/// Consume `\u{XXX}`, with the cursor on the `u`.
///
/// # Errors
///
/// Returns a [`LexError`] for missing braces, no digits, or a codepoint
/// `version` doesn't allow.
fn scan_unicode_escape(cursor: &mut Cursor<'_>, version: LuaVersion) -> Result<(), LexError> {
    let escape_start = cursor.position().saturating_sub(1);
    cursor.advance_by(1);
    if !cursor.eat(b'{') {
        return Err(lex_error(cursor.span_since(escape_start), "`\\u` escape requires `{`"));
    }
    let hex_start = cursor.position();
    cursor.eat_while(|b| b.is_ascii_hexdigit());
    let hex_digits = cursor.text_since(hex_start);
    if hex_digits.is_empty() {
        return Err(lex_error(
            cursor.span_since(escape_start),
            "`\\u{}` requires at least one hex digit",
        ));
    }
    if unicode_escape_codepoint(hex_digits.as_bytes(), version).is_none() {
        let limit = if version.has_extended_unicode_escape() { "2^31" } else { "0x110000" };
        return Err(lex_error(
            cursor.span_since(escape_start),
            format!("`\\u` codepoint too large (must be less than {limit})"),
        ));
    }
    if !cursor.eat(b'}') {
        return Err(lex_error(cursor.span_since(escape_start), "`\\u` escape missing closing `}`"));
    }
    Ok(())
}

/// Consume the escaped `character` at the cursor and build the error for its
/// undefined escape.
fn invalid_escape(cursor: &mut Cursor<'_>, character: u8) -> LexError {
    let character_start = cursor.position();
    cursor.advance_by(1);
    let escaped = cursor.eat_rest_of_char(character_start).unwrap_or_else(|| char::from(character));
    lex_error(
        cursor.span_since(character_start.saturating_sub(1)),
        format!("invalid escape sequence `\\{escaped}`"),
    )
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
    errors: &mut Vec<LexError>,
) -> bool {
    let body_start = cursor.position();
    loop {
        let rest = cursor.rest();
        let Some(bracket_offset) = memchr::memchr(b']', rest) else {
            cursor.advance_by(rest.len());
            errors.push(lex_error(cursor.span_since(start), unterminated));
            return false;
        };
        cursor.advance_by(bracket_offset.saturating_add(1));
        let after_bracket = cursor.rest();
        let closing_level = after_bracket.iter().take_while(|&&b| b == b'=').count();
        if closing_level == level && after_bracket.get(level) == Some(&b']') {
            cursor.advance_by(level.saturating_add(1));
            if level == 0
                && version.has_long_bracket_nesting_error()
                && let Some(offset) = memmem::find(cursor.text_since(body_start).as_bytes(), b"[[")
            {
                let nested_open = body_start.saturating_add(offset);
                errors.push(lex_error(
                    Span::from_indices(nested_open, nested_open.saturating_add(2)),
                    "nesting of `[[...]]` is deprecated",
                ));
            }
            return true;
        }
    }
}
