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
        [b'0', b'x' | b'X', ..] => scan_hex_number(cursor, version, start),
        // Luau
        [b'0', b'b' | b'B', ..] if version.has_binary_literals() => {
            scan_binary_number(cursor, version, start)
        }
        _ => scan_decimal_number(cursor, version, start),
    }
}

/// # Errors
///
/// Returns a [`LexError`] for a malformed hexadecimal numeral.
fn scan_hex_number(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
) -> Result<TokenKind, LexError> {
    cursor.advance_by(2); // `0x`

    let digits_start = cursor.position();
    eat_digits(cursor, version, u8::is_ascii_hexdigit);
    // Underscores are separators, not digits, so `0x_` carries no value.
    let has_integer_part =
        cursor.text_since(digits_start).bytes().any(|byte| byte.is_ascii_hexdigit());

    if version.has_hex_floats() {
        // Lua 5.2+
        if !has_integer_part && cursor.peek() != Some(b'.') {
            return Err(lex_error(
                cursor.span_since(start),
                "hex literal requires at least one digit after `0x`",
            ));
        }
        if cursor.eat(b'.') {
            let fraction_start = cursor.position();
            eat_digits(cursor, version, u8::is_ascii_hexdigit);
            if !has_integer_part && cursor.position() == fraction_start {
                return Err(lex_error(
                    cursor.span_since(start),
                    "hex float requires digits before or after decimal point",
                ));
            }
        }
        if matches!(cursor.peek(), Some(b'p' | b'P')) && !eat_exponent(cursor, version) {
            return Err(lex_error(cursor.span_since(start), "hex float exponent requires digits"));
        }
        if cursor.peek() == Some(b'.') {
            return Err(lex_error(
                Span::from_indices(start, cursor.position().saturating_add(1)),
                "malformed hexadecimal number",
            ));
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

    // Luau's hex scanner consumes dots and 5.1's stops before them. 5.2+
    // already failed in the hex-float path.
    if version.is_luau() {
        reject_adjacent_concat(cursor, start)?;
    }

    if version.has_luau_integer_literals() && cursor.eat(b'i') {
        validate_integer_range(cursor, start, 16)?;
    }

    // Every Lua scanner swallows the letters trailing a hex numeral and then
    // fails to convert them, so `0xFF.foo` is one malformed number rather
    // than `0xFF.f` followed by the name `oo`.
    if cursor.peek().is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_') {
        return Err(lex_error(
            Span::from_indices(start, cursor.position().saturating_add(1)),
            "malformed hexadecimal number",
        ));
    }

    finish_number(cursor, version, start)
}

/// # Errors
///
/// Returns a [`LexError`] for a malformed binary numeral.
fn scan_binary_number(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
) -> Result<TokenKind, LexError> {
    cursor.advance_by(2); // `0b`

    let digits_start = cursor.position();
    eat_digits(cursor, version, |byte| matches!(byte, b'0' | b'1'));
    if cursor.position() == digits_start {
        return Err(lex_error(
            cursor.span_since(start),
            "binary literal requires at least one digit",
        ));
    }

    // Binary literals are Luau-only, and Luau's scanner consumes dots.
    reject_adjacent_concat(cursor, start)?;

    if version.has_luau_integer_literals() && cursor.eat(b'i') {
        validate_integer_range(cursor, start, 2)?;
    }

    finish_number(cursor, version, start)
}

/// # Errors
///
/// Returns a [`LexError`] for a malformed decimal numeral.
fn scan_decimal_number(
    cursor: &mut Cursor<'_>,
    version: LuaVersion,
    start: usize,
) -> Result<TokenKind, LexError> {
    eat_digits(cursor, version, u8::is_ascii_digit);

    let mut is_float = false;
    if cursor.peek() == Some(b'.') {
        let should_consume_dot = match cursor.peek_at(1) {
            Some(byte) if byte.is_ascii_digit() => true,
            Some(b'e' | b'E') => true,
            Some(b'.') => false, // `..` or `...`
            Some(byte) if byte.is_ascii_alphabetic() || byte == b'_' => false, // field access like `1.foo`
            _ => true, // `1.` is a valid float at EOF or before an operator
        };
        if should_consume_dot {
            is_float = true;
            cursor.advance(); // .
            eat_digits(cursor, version, u8::is_ascii_digit);
        }
    }

    if matches!(cursor.peek(), Some(b'e' | b'E')) {
        is_float = true;
        if !eat_exponent(cursor, version) {
            return Err(lex_error(cursor.span_since(start), "decimal exponent requires digits"));
        }
    }

    reject_adjacent_concat(cursor, start)?;

    if version.has_luau_integer_literals() && cursor.eat(b'i') {
        // Luau consumes the suffix on floats too: `2.5i` is a malformed
        // integer, not a float followed by an identifier.
        if is_float {
            return Err(lex_error(
                cursor.span_since(start),
                "the `i` integer suffix is not allowed on float literals",
            ));
        }
        validate_integer_range(cursor, start, 10)?;
    }

    finish_number(cursor, version, start)
}

/// Consume digits that satisfy `is_digit`, plus underscore separators where
/// `version` has them.
fn eat_digits(cursor: &mut Cursor<'_>, version: LuaVersion, is_digit: impl Fn(&u8) -> bool) {
    if version.has_underscore_separators() {
        cursor.eat_while(|byte| is_digit(&byte) || byte == b'_');
    } else {
        cursor.eat_while(|byte| is_digit(&byte));
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
    let digits_start = cursor.position();
    eat_digits(cursor, version, u8::is_ascii_digit);
    cursor.position() > digits_start
}

/// Reject a numeral running straight into `..`.
///
/// Every real Lua numeral scanner consumes `.` greedily, so that is
/// "malformed number", never a concat (`1 ..2` needs the space). 5.1's hex
/// scanner is the exception; see `scan_hex_number`.
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

/// Build the token for the numeral ending at the cursor.
///
/// Rejects underscore separators outside Luau. Luau itself strips underscores
/// anywhere in the literal before conversion, so `0b_01`, `1__2`, and `12_`
/// are all valid there, with no placement rules.
///
/// # Errors
///
/// Returns a [`LexError`] for an underscore when `version` has no separators.
fn finish_number(
    cursor: &Cursor<'_>,
    version: LuaVersion,
    start: usize,
) -> Result<TokenKind, LexError> {
    let raw = cursor.text_since(start);
    if raw.contains('_') && !version.has_underscore_separators() {
        return Err(lex_error(
            cursor.span_since(start),
            "underscore separators in numbers are not supported in this Lua version",
        ));
    }
    Ok(TokenKind::Number(raw.into()))
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
    let spelled_digits = if radix == 10 { raw } else { raw.get(2..).unwrap_or_default() };
    let normalized;
    let digits = if spelled_digits.contains('_') {
        normalized =
            spelled_digits.chars().filter(|character| *character != '_').collect::<String>();
        normalized.as_str()
    } else {
        spelled_digits
    };
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
