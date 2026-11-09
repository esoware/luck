use luck_token::{LuaVersion, Span, TokenKind};

use crate::cursor::Cursor;
use crate::lex_error::{LexError, lex_error};

/// Scan a number literal, with the cursor on its first digit or on a `.`
/// followed by a digit.
///
/// # Errors
///
/// Returns a [`LexError`] for a malformed numeral.
pub(crate) fn scan_number(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
) -> Result<TokenKind, LexError> {
    let start = cursor.position();
    match cursor.rest() {
        // Every reference scanner reads the digit after a leading `.` as the
        // numeral's first, so `.0x` opens a hex scan that can never convert.
        [b'.', b'0', b'x' | b'X', ..] => {
            cursor.advance_by(2);
            return Err(touching_letter_error(cursor, start));
        }
        [b'0', after_zero @ ..] => {
            // Luau strips every `_` before conversion, so `0_x1` is hex.
            let underscore_count = if version.has_underscore_separators() {
                after_zero.iter().take_while(|&&byte| byte == b'_').count()
            } else {
                0
            };
            let prefix_length = underscore_count.saturating_add(2);
            match after_zero.get(underscore_count) {
                Some(b'x' | b'X') => scan_hex_number(cursor, version, start, prefix_length)?,
                // Luau
                Some(b'b' | b'B') if version.has_binary_literals() => {
                    scan_binary_number(cursor, version, start, prefix_length)?;
                }
                _ => scan_decimal_number(cursor, version, start)?,
            }
        }
        _ => scan_decimal_number(cursor, version, start)?,
    }
    Ok(TokenKind::Number(cursor.text_since(start).into()))
}

/// # Errors
///
/// Returns a [`LexError`] for a malformed hexadecimal numeral.
fn scan_hex_number(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
    prefix_length: usize,
) -> Result<(), LexError> {
    cursor.advance_by(prefix_length);

    let has_integer_part = eat_digits(cursor, version, u8::is_ascii_hexdigit);

    if version.has_hex_floats() {
        // Lua 5.2+
        if !has_integer_part && cursor.peek() != Some(b'.') {
            return Err(lex_error(
                cursor.span_since(start),
                "hex literal requires at least one digit after `0x`",
            ));
        }
        if cursor.eat(b'.') {
            let has_fraction = eat_digits(cursor, version, u8::is_ascii_hexdigit);
            if !has_integer_part && !has_fraction {
                return Err(lex_error(
                    cursor.span_since(start),
                    "hex float requires digits before or after decimal point",
                ));
            }
        }
        if matches!(cursor.peek(), Some(b'p' | b'P')) && !eat_exponent(cursor, version) {
            return Err(lex_error(cursor.span_since(start), "hex float exponent requires digits"));
        }
    } else {
        // Without hex floats, a `.` can't start a fraction, so a digitless
        // `0x` is malformed whatever follows it.
        if !has_integer_part {
            return Err(lex_error(
                cursor.span_since(start),
                "hex literal requires at least one digit after `0x`",
            ));
        }
        if matches!(cursor.rest(), [b'.', digit, ..] if digit.is_ascii_hexdigit())
            || matches!(cursor.peek(), Some(b'p' | b'P'))
        {
            return Err(lex_error(
                cursor.span_since(start),
                "hex float literals are not supported in this Lua version",
            ));
        }
    }

    eat_integer_suffix(cursor, version, start, 16)?;

    if (version.has_hex_scan_dots() && cursor.peek() == Some(b'.'))
        || is_touching_numeral(cursor, version)
    {
        return Err(lex_error(
            Span::from_indices(start, cursor.position().saturating_add(1)),
            "malformed hexadecimal number",
        ));
    }
    Ok(())
}

/// # Errors
///
/// Returns a [`LexError`] for a malformed binary numeral.
fn scan_binary_number(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
    prefix_length: usize,
) -> Result<(), LexError> {
    cursor.advance_by(prefix_length);

    if !eat_digits(cursor, version, |byte| matches!(byte, b'0' | b'1')) {
        return Err(lex_error(
            cursor.span_since(start),
            "binary literal requires at least one digit",
        ));
    }

    eat_integer_suffix(cursor, version, start, 2)?;

    if is_touching_numeral(cursor, version) {
        return Err(touching_letter_error(cursor, start));
    }
    Ok(())
}

/// # Errors
///
/// Returns a [`LexError`] for a malformed decimal numeral.
fn scan_decimal_number(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
) -> Result<(), LexError> {
    eat_digits(cursor, version, u8::is_ascii_digit);

    let mut is_float = false;
    if cursor.peek() == Some(b'.') && cursor.peek_next() != Some(b'.') {
        is_float = true;
        cursor.advance();
        eat_digits(cursor, version, u8::is_ascii_digit);
    }

    let has_exponent = matches!(cursor.peek(), Some(b'e' | b'E'));
    if has_exponent {
        is_float = true;
        if !eat_exponent(cursor, version) {
            return Err(lex_error(cursor.span_since(start), "decimal exponent requires digits"));
        }
    }

    if !has_exponent || version.has_hex_scan_dots() {
        reject_adjacent_concat(cursor, start)?;
    }

    // Luau consumes the suffix on floats too: `2.5i` is a malformed integer,
    // not a float followed by an identifier.
    if is_float && version.has_luau_integer_literals() && cursor.eat(b'i') {
        return Err(lex_error(
            cursor.span_since(start),
            "the `i` integer suffix is not allowed on float literals",
        ));
    }
    eat_integer_suffix(cursor, version, start, 10)?;

    if is_touching_numeral(cursor, version) {
        return Err(touching_letter_error(cursor, start));
    }
    Ok(())
}

/// Whether the byte at the cursor would join the numeral in `version`'s
/// scanner and leave it unconvertible. 5.2 and 5.3 only read on through hex
/// digits, so any other letter starts a new token there.
fn is_touching_numeral(cursor: &Cursor<'_>, version: LuaVersion) -> bool {
    cursor.peek().is_some_and(|byte| {
        if version.has_strict_numeral_end() {
            byte.is_ascii_alphanumeric() || byte == b'_'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

/// A malformed-number error spanning the numeral through the letter at the
/// cursor.
fn touching_letter_error(cursor: &Cursor<'_>, start: usize) -> LexError {
    lex_error(
        Span::from_indices(start, cursor.position().saturating_add(1)),
        "malformed number (a numeral cannot touch a letter)",
    )
}

/// Consume digits that satisfy `is_digit`, plus underscore separators where
/// `version` has them, and return whether any digit was among them.
///
/// Elsewhere an underscore ends the digits. Luau strips underscores
/// anywhere in the literal before conversion, so `0b_01`, `1__2`, and `12_`
/// are all valid there, with no placement rules.
fn eat_digits(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    is_digit: impl Fn(&u8) -> bool,
) -> bool {
    let digits_start = cursor.position();
    if version.has_underscore_separators() {
        cursor.eat_while(|byte| is_digit(&byte) || byte == b'_');
        cursor.text_since(digits_start).bytes().any(|byte| byte != b'_')
    } else {
        cursor.eat_while(|byte| is_digit(&byte));
        cursor.position() > digits_start
    }
}

/// Consume an exponent with the cursor on its marker (`e` or `p`): the
/// marker, an optional sign, and the decimal digits. Returns whether any
/// digits followed.
fn eat_exponent(cursor: &mut Cursor<'_>, version: LuaVersion) -> bool {
    cursor.advance();
    if matches!(cursor.peek(), Some(b'+' | b'-')) {
        cursor.advance();
    }
    eat_digits(cursor, version, u8::is_ascii_digit)
}

/// Consume a Luau `i` suffix and any `_` after it, range-checking the integer.
///
/// # Errors
///
/// Returns a [`LexError`] when the value doesn't fit in 64 bits.
fn eat_integer_suffix(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
    radix: u32,
) -> Result<(), LexError> {
    if version.has_luau_integer_literals() && cursor.eat(b'i') {
        validate_integer_range(cursor, start, radix)?;
        cursor.eat_while(|byte| byte == b'_');
    }
    Ok(())
}

/// Reject a decimal numeral running straight into `..`.
///
/// Every reference scanner consumes `.` greedily up to an exponent, so that
/// is "malformed number", never a concat (`1 ..2` needs the space). Past an
/// exponent only the 5.2+ scanners still do.
///
/// # Errors
///
/// Returns a [`LexError`] spanning the numeral and the two dots.
fn reject_adjacent_concat(cursor: &Cursor<'_>, start: usize) -> Result<(), LexError> {
    if cursor.rest().starts_with(b"..") {
        return Err(lex_error(
            Span::from_indices(start, cursor.position().saturating_add(2)),
            "malformed number (a numeral cannot directly precede `..`)",
        ));
    }
    Ok(())
}

/// Range-check a numeral ending at the cursor with its `i` suffix consumed.
///
/// A decimal literal must fit an `i64`, while hexadecimal and binary ones may
/// use all 64 bits, matching Luau's bit-pattern semantics.
///
/// # Errors
///
/// Returns a [`LexError`] when the value doesn't fit.
fn validate_integer_range(cursor: &Cursor<'_>, start: usize, radix: u32) -> Result<(), LexError> {
    let text = cursor.text_since(start);
    let raw = text.strip_suffix('i').unwrap_or(text);
    let normalized;
    let spelled = if raw.contains('_') {
        normalized = raw.replace('_', "");
        normalized.as_str()
    } else {
        raw
    };
    let digits = if radix == 10 { spelled } else { spelled.get(2..).unwrap_or_default() };
    let is_valid = if radix == 10 {
        digits.parse::<i64>().is_ok()
    } else {
        u64::from_str_radix(digits, radix).is_ok()
    };

    if is_valid {
        Ok(())
    } else {
        Err(lex_error(cursor.span_since(start), "integer literal is outside the 64-bit range"))
    }
}
