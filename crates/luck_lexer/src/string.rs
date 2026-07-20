use crate::cursor::Cursor;
use crate::search::{ByteMatchTable, byte_match_table};
use crate::{LexError, lex_error};
use luck_token::{LuaVersion, Span, TokenKind};

// Stop bytes for the short-string body scan: either quote form (the
// non-delimiter quote falls through as ordinary content), escapes, and
// the line breaks that make a string unterminated.
static SHORT_STRING_STOP: ByteMatchTable =
    byte_match_table!(|byte| matches!(byte, b'"' | b'\'' | b'\\' | b'\n' | b'\r'));

/// Lex a single-quoted or double-quoted string. Cursor should be positioned at the opening quote.
pub fn lex_short_string(
    cursor: &mut Cursor,
    source: &str,
    version: LuaVersion,
) -> Result<TokenKind, LexError> {
    let start = cursor.position();
    let quote = cursor
        .advance()
        .expect("caller confirmed quote char at cursor");

    loop {
        match cursor.peek() {
            None | Some(b'\n' | b'\r') => {
                return Err(crate::lex_error(
                    Span::new(start as u32, cursor.position() as u32),
                    "unterminated string",
                ));
            }
            Some(b'\\') => {
                cursor.advance();
                match cursor.peek() {
                    None => {
                        return Err(crate::lex_error(
                            Span::new(start as u32, cursor.position() as u32),
                            "unterminated string",
                        ));
                    }
                    Some(b'\n') => {
                        cursor.advance();
                        // PUC treats LFCR as one EOL sequence after `\`.
                        if !version.is_luau() && cursor.peek() == Some(b'\r') {
                            cursor.advance();
                        }
                    }
                    Some(b'\r') => {
                        cursor.advance();
                        if cursor.peek() == Some(b'\n') {
                            cursor.advance();
                        }
                    }
                    Some(ch) if ch.is_ascii_digit() => {
                        let escape_start = cursor.position() - 1;
                        let mut value: u32 = 0;
                        let mut count = 0;
                        while count < 3 {
                            match cursor.peek() {
                                Some(d) if d.is_ascii_digit() => {
                                    value = value * 10 + (d - b'0') as u32;
                                    cursor.advance();
                                    count += 1;
                                }
                                _ => break,
                            }
                        }
                        if value > 255 {
                            return Err(crate::lex_error(
                                Span::new(escape_start as u32, cursor.position() as u32),
                                format!("decimal escape too large (\\{value}), maximum is \\255"),
                            ));
                        }
                    }
                    Some(b'x') if version.has_hex_escape() => {
                        let escape_start = cursor.position() - 1;
                        cursor.advance();
                        for i in 0..2 {
                            match cursor.peek() {
                                Some(d) if d.is_ascii_hexdigit() => {
                                    cursor.advance();
                                }
                                _ => {
                                    return Err(crate::lex_error(
                                        Span::new(escape_start as u32, cursor.position() as u32),
                                        format!(
                                            "\\x escape requires exactly 2 hex digits, got {}",
                                            i
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                    Some(b'z') if version.has_whitespace_escape() => {
                        cursor.advance();
                        while let Some(b) = cursor.peek() {
                            if matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0B' | b'\x0C') {
                                cursor.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    Some(b'u') if version.has_unicode_escape() => {
                        let escape_start = cursor.position() - 1;
                        cursor.advance();
                        if cursor.peek() != Some(b'{') {
                            return Err(crate::lex_error(
                                Span::new(escape_start as u32, cursor.position() as u32),
                                "\\u escape requires '{'",
                            ));
                        }
                        cursor.advance();
                        let hex_start = cursor.position();
                        let mut hex_count = 0;
                        while let Some(d) = cursor.peek() {
                            if d.is_ascii_hexdigit() {
                                cursor.advance();
                                hex_count += 1;
                            } else {
                                break;
                            }
                        }
                        if hex_count == 0 {
                            return Err(crate::lex_error(
                                Span::new(escape_start as u32, cursor.position() as u32),
                                "\\u{} requires at least one hex digit",
                            ));
                        }
                        let hex_str = &source[hex_start..cursor.position()];
                        if let Ok(codepoint) = u64::from_str_radix(hex_str, 16) {
                            if codepoint >= 0x80000000 {
                                return Err(crate::lex_error(
                                    Span::new(escape_start as u32, cursor.position() as u32),
                                    "\\u codepoint too large (must be less than 2^31)",
                                ));
                            }
                        } else {
                            return Err(crate::lex_error(
                                Span::new(escape_start as u32, cursor.position() as u32),
                                "\\u codepoint too large (must be less than 2^31)",
                            ));
                        }
                        if cursor.peek() != Some(b'}') {
                            return Err(crate::lex_error(
                                Span::new(escape_start as u32, cursor.position() as u32),
                                "\\u escape missing closing '}'",
                            ));
                        }
                        cursor.advance();
                    }
                    Some(b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'\'' | b'"') => {
                        cursor.advance();
                    }
                    // Lua 5.1 accepts any other escaped character as that
                    // literal character; 5.2+ and Luau reject it.
                    Some(_) if !version.has_strict_escapes() => {
                        cursor.advance();
                    }
                    Some(ch) => {
                        let escape_start = cursor.position() - 1;
                        cursor.advance();
                        return Err(crate::lex_error(
                            Span::new(escape_start as u32, cursor.position() as u32),
                            format!("invalid escape sequence '\\{}'", ch as char),
                        ));
                    }
                }
            }
            Some(ch) if ch == quote => {
                cursor.advance();
                let raw = &source[start..cursor.position()];
                return Ok(TokenKind::StringLiteral(raw.into()));
            }
            Some(_) => {
                cursor.advance();
                cursor.advance_until_match(&SHORT_STRING_STOP);
            }
        }
    }
}

/// Level of a long-bracket opener at the cursor (`[==[` is level 2),
/// without consuming. `None` when the cursor is not at `[=*[`.
pub fn long_bracket_level(cursor: &Cursor) -> Option<usize> {
    if cursor.peek() != Some(b'[') {
        return None;
    }

    let mut offset = 1;
    let mut level = 0;
    while cursor.peek_at(offset) == Some(b'=') {
        offset += 1;
        level += 1;
    }

    (cursor.peek_at(offset) == Some(b'[')).then_some(level)
}

/// Advance cursor past a long bracket opening `[=*[` of known level.
pub fn skip_long_bracket_open(cursor: &mut Cursor, level: usize) {
    cursor.advance(); // [
    for _ in 0..level {
        cursor.advance(); // =
    }
    cursor.advance(); // [
}

/// Scan from just past a long-bracket opener to its matching `]=*]`
/// closer of `level`, consuming the closer. Returns `true` when the
/// closer was found; on EOF it consumes the remainder and returns
/// `false` so the caller can raise a context-specific unterminated
/// error. Shared by long-bracket strings and block comments.
pub fn scan_to_long_bracket_close(cursor: &mut Cursor, level: usize) -> bool {
    loop {
        let rest = cursor.rest();
        let Some(bracket_offset) = memchr::memchr(b']', rest) else {
            cursor.advance_by(rest.len());
            return false;
        };
        cursor.advance_by(bracket_offset);
        let mut closing_level = 0;
        let mut offset = 1;
        while cursor.peek_at(offset) == Some(b'=') {
            closing_level += 1;
            offset += 1;
        }
        if closing_level == level && cursor.peek_at(offset) == Some(b']') {
            cursor.advance_by(offset + 1);
            return true;
        }
        cursor.advance();
    }
}

/// Lex a long-bracket string body of known `level`, from a cursor
/// positioned just past the `[=*[` opener.
pub fn lex_long_bracket_body(
    cursor: &mut Cursor,
    source: &str,
    start: usize,
    level: usize,
) -> Result<TokenKind, LexError> {
    if !scan_to_long_bracket_close(cursor, level) {
        return Err(lex_error(
            Span::new(start as u32, cursor.position() as u32),
            "unterminated long bracket string",
        ));
    }
    let raw = &source[start..cursor.position()];
    Ok(TokenKind::StringLiteral(raw.into()))
}
