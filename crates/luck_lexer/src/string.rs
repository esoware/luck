use luck_token::{CompactString, LuaVersion, Span, TokenKind};

use crate::cursor::Cursor;
use crate::lex_error::{LexError, lex_error};
use crate::search::{ByteMatchTable, byte_match_table, find_match};

// Stop bytes for the short-string body scan: either quote form (the
// non-delimiter quote falls through as ordinary content), escapes, and
// the line breaks that make a string unterminated.
static SHORT_STRING_STOP: ByteMatchTable =
    byte_match_table!(|byte| matches!(byte, b'"' | b'\'' | b'\\' | b'\n' | b'\r'));

// Stop bytes for raw interpolated-string text: escape lead, interpolation
// opener, closing backtick, and the line breaks Luau rejects.
static INTERPOLATED_STRING_STOP: ByteMatchTable =
    byte_match_table!(|byte| matches!(byte, b'\\' | b'{' | b'`' | b'\n' | b'\r'));

/// Scan a single- or double-quoted string, with the cursor on the opening
/// `quote`.
///
/// # Errors
///
/// Returns a [`LexError`] for an unterminated string or a malformed escape.
pub(crate) fn scan_short_string(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    quote: u8,
) -> Result<TokenKind, LexError> {
    let start = cursor.position();
    cursor.advance();

    loop {
        match cursor.peek() {
            None | Some(b'\n' | b'\r') => {
                return Err(lex_error(cursor.span_since(start), "unterminated string"));
            }
            Some(b'\\') => {
                cursor.advance();
                let Some(escaped) = cursor.peek() else {
                    return Err(lex_error(cursor.span_since(start), "unterminated string"));
                };
                scan_escape(cursor, version, escaped)?;
            }
            Some(character) if character == quote => {
                cursor.advance();
                return Ok(TokenKind::StringLiteral(cursor.text_since(start).into()));
            }
            Some(_) => {
                cursor.advance();
                // Most strings end within 8 bytes, so the table checks those
                // before `memchr` takes over.
                let bytes = cursor.rest();
                let (prefix, rest) = bytes.split_at(bytes.len().min(8));
                let prefix_stop = find_match(prefix, &SHORT_STRING_STOP);
                cursor.advance_by(prefix_stop);
                if prefix_stop == prefix.len() {
                    // Stopping at LF bounds an unterminated string to its
                    // line; the second scan finds any CR before that stop.
                    let end = memchr::memchr3(quote, b'\\', b'\n', rest).unwrap_or(rest.len());
                    let (line, _) = rest.split_at(end);
                    cursor.advance_by(memchr::memchr(b'\r', line).unwrap_or(end));
                }
            }
        }
    }
}

/// How a raw interpolated-string segment ended.
pub(crate) enum InterpolatedSegmentEnd {
    /// `{`, so an interpolation expression follows.
    OpenBrace,
    /// `` ` ``, so the string is complete.
    Backtick,
}

/// Scan raw interpolated-string text up to an interpolation opener `{` or
/// the closing backtick, consuming that delimiter. `start` is where the
/// token began, at its backtick or the `}` that resumed the string.
///
/// Raw escapes stay intact, so the validated segment is copied once from the
/// source.
///
/// # Errors
///
/// Returns a [`LexError`] for an unterminated string, a malformed escape, or
/// a `{{`.
pub(crate) fn scan_interpolated_segment(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
) -> Result<(CompactString, InterpolatedSegmentEnd), LexError> {
    let segment_start = cursor.position();
    loop {
        match cursor.peek() {
            // Luau rejects unescaped line breaks in backtick strings, same as
            // in short strings.
            None | Some(b'\n' | b'\r') => {
                return Err(lex_error(
                    cursor.span_since(start),
                    "unterminated interpolated string",
                ));
            }
            Some(b'\\') => {
                cursor.advance();
                match cursor.peek() {
                    // Escaped braces and backticks stay literal text.
                    Some(b'{' | b'}' | b'`') => {
                        cursor.advance();
                    }
                    // Consuming `\u{...}` whole also keeps its `{` from
                    // opening an interpolation.
                    Some(escaped) => scan_escape(cursor, version, escaped)?,
                    // The next iteration reports the unterminated string.
                    None => {}
                }
            }
            Some(b'{') => {
                let text = cursor.text_since(segment_start).into();
                cursor.advance();
                if cursor.peek() == Some(b'{') {
                    let position = cursor.position();
                    return Err(lex_error(
                        Span::from_indices(position.saturating_sub(1), position.saturating_add(1)),
                        "`{{` is not allowed in interpolated strings, use `\\{` for a literal brace",
                    ));
                }
                return Ok((text, InterpolatedSegmentEnd::OpenBrace));
            }
            Some(b'`') => {
                let text = cursor.text_since(segment_start).into();
                cursor.advance();
                return Ok((text, InterpolatedSegmentEnd::Backtick));
            }
            Some(_) => {
                cursor.advance();
                cursor.advance_by(find_match(cursor.rest(), &INTERPOLATED_STRING_STOP));
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
            cursor.advance();
            if version.has_lfcr_line_break() && cursor.peek() == Some(b'\r') {
                cursor.advance();
            }
        }
        b'\r' => {
            cursor.advance();
            if cursor.peek() == Some(b'\n') {
                cursor.advance();
            }
        }
        character if character.is_ascii_digit() => scan_decimal_escape(cursor)?,
        b'x' if version.has_hex_escape() => scan_hex_escape(cursor)?,
        b'z' if version.has_whitespace_escape() => {
            cursor.advance();
            cursor
                .eat_while(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0B' | b'\x0C'));
        }
        b'u' if version.has_unicode_escape() => scan_unicode_escape(cursor)?,
        b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'\'' | b'"' => {
            cursor.advance();
        }
        // Lua 5.1 accepts any other escaped character as that
        // literal character; 5.2+ and Luau reject it.
        _ if !version.has_strict_escapes() => {
            cursor.advance();
        }
        character => return Err(invalid_escape(cursor, character)),
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
    let digit_count = cursor.rest().iter().take(3).take_while(|byte| byte.is_ascii_digit()).count();
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
    cursor.advance();
    for digit_count in 0..2u32 {
        match cursor.peek() {
            Some(digit) if digit.is_ascii_hexdigit() => {
                cursor.advance();
            }
            _ => {
                return Err(lex_error(
                    cursor.span_since(escape_start),
                    format!("`\\x` escape requires exactly 2 hex digits, got {digit_count}"),
                ));
            }
        }
    }
    Ok(())
}

/// Consume `\u{XXX}`, with the cursor on the `u`.
///
/// # Errors
///
/// Returns a [`LexError`] for missing braces, no digits, or a codepoint of
/// 2^31 or above.
fn scan_unicode_escape(cursor: &mut Cursor<'_>) -> Result<(), LexError> {
    let escape_start = cursor.position().saturating_sub(1);
    cursor.advance();
    if !cursor.eat(b'{') {
        return Err(lex_error(cursor.span_since(escape_start), "`\\u` escape requires `{`"));
    }
    let hex_start = cursor.position();
    cursor.eat_while(|byte| byte.is_ascii_hexdigit());
    let hex_digits = cursor.text_since(hex_start);
    if hex_digits.is_empty() {
        return Err(lex_error(
            cursor.span_since(escape_start),
            "`\\u{}` requires at least one hex digit",
        ));
    }
    if !u64::from_str_radix(hex_digits, 16).is_ok_and(|codepoint| codepoint < 0x8000_0000) {
        return Err(lex_error(
            cursor.span_since(escape_start),
            "`\\u` codepoint too large (must be less than 2^31)",
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
    cursor.advance();
    let escaped = cursor.eat_rest_of_char(character_start).unwrap_or_else(|| char::from(character));
    lex_error(
        cursor.span_since(character_start.saturating_sub(1)),
        format!("invalid escape sequence `\\{escaped}`"),
    )
}

/// Consume a long-bracket opener `[=*[` at the cursor and return its level
/// (`[==[` is level 2). Consumes nothing and returns `None` when the cursor
/// isn't at one.
pub(crate) fn eat_long_bracket_open(cursor: &mut Cursor<'_>) -> Option<usize> {
    let after_bracket = cursor.rest().strip_prefix(b"[")?;
    let level = after_bracket.iter().take_while(|&&byte| byte == b'=').count();
    if after_bracket.get(level) != Some(&b'[') {
        return None;
    }
    cursor.advance_by(level.saturating_add(2));
    Some(level)
}

/// Advance from just past a long-bracket opener to its matching `]=*]` closer.
///
/// Consumes the closer and returns `true` when it's found. At EOF it consumes
/// the rest and returns `false`, so the caller can report its own
/// unterminated error. Shared by long-bracket strings and block comments.
pub(crate) fn advance_past_long_bracket_close(cursor: &mut Cursor<'_>, level: usize) -> bool {
    loop {
        let rest = cursor.rest();
        let Some(bracket_offset) = memchr::memchr(b']', rest) else {
            cursor.advance_by(rest.len());
            return false;
        };
        cursor.advance_by(bracket_offset);
        cursor.advance();
        let after_bracket = cursor.rest();
        let closing_level = after_bracket.iter().take_while(|&&byte| byte == b'=').count();
        if closing_level == level && after_bracket.get(level) == Some(&b']') {
            cursor.advance_by(level);
            cursor.advance();
            return true;
        }
    }
}

/// Scan a long-bracket string of known `level` that opened at `start`, from
/// a cursor just past its `[=*[` opener.
///
/// # Errors
///
/// Returns a [`LexError`] when the closing bracket is missing.
pub(crate) fn scan_long_string(
    cursor: &mut Cursor<'_>,
    start: usize,
    level: usize,
) -> Result<TokenKind, LexError> {
    if !advance_past_long_bracket_close(cursor, level) {
        return Err(lex_error(cursor.span_since(start), "unterminated long bracket string"));
    }
    Ok(TokenKind::StringLiteral(cursor.text_since(start).into()))
}
