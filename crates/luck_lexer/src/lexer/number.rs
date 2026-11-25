use luck_diagnostics::Diagnostic;
use luck_token::{Span, TokenKind};

use super::Lexer;
use crate::cursor::Cursor;

impl Lexer<'_> {
    /// Lex a numeral that starts at `start` with `first`, a digit or a `.`
    /// before one, with the cursor past `first`, or report a malformed one
    /// and return `None`.
    #[inline]
    pub(super) fn lex_number(&mut self, start: usize, first: u8) -> Option<TokenKind> {
        let rest = self.cursor.rest();
        let digit_count = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        // Most numerals are decimal integers that nothing else touches.
        if first != b'.'
            && !rest
                .get(digit_count)
                .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        {
            self.cursor.advance_by(digit_count);
            return Some(TokenKind::Number);
        }
        match scan_other_number(&mut self.cursor, start, first) {
            Ok(()) => Some(TokenKind::Number),
            Err(error) => {
                self.errors.push(error);
                None
            }
        }
    }
}

/// Cold and out of line, which parses 1-2% faster than leaving it to the
/// compiler.
#[cold]
#[inline(never)]
fn scan_other_number(cursor: &mut Cursor<'_>, start: usize, first: u8) -> Result<(), Diagnostic> {
    if first == b'0' {
        // Luau strips every `_` before conversion, so `0_x1` is hex.
        let after_zero = cursor.rest();
        let underscore_count = after_zero.iter().take_while(|&&b| b == b'_').count();
        let prefix_length = underscore_count + 1;
        match after_zero.get(underscore_count) {
            Some(b'x' | b'X') => scan_hex_number(cursor, start, prefix_length),
            Some(b'b' | b'B') => scan_binary_number(cursor, start, prefix_length),
            _ => scan_decimal_number(cursor, start, false),
        }
    } else {
        scan_decimal_number(cursor, start, first == b'.')
    }
}

/// The cursor is past the `0`, `prefix_length` bytes before the hex digits.
fn scan_hex_number(
    cursor: &mut Cursor<'_>,
    start: usize,
    prefix_length: usize,
) -> Result<(), Diagnostic> {
    cursor.advance_by(prefix_length);

    if !eat_digits(cursor, u8::is_ascii_hexdigit) {
        return Err(Diagnostic::e008(
            cursor.span_since(start),
            "hex literal requires at least one digit after `0x`",
        ));
    }

    // Luau's scanner ends a hexadecimal numeral at a `.`, but one followed by
    // a hex digit is a fraction that was meant to join it.
    if is_touching_numeral(cursor)
        || matches!(cursor.rest(), [b'.', digit, ..] if digit.is_ascii_hexdigit())
    {
        return Err(Diagnostic::e008(
            Span::from_indices(start, cursor.position() + 1),
            "malformed hexadecimal number",
        ));
    }
    Ok(())
}

/// The cursor is past the `0`, `prefix_length` bytes before the binary
/// digits.
fn scan_binary_number(
    cursor: &mut Cursor<'_>,
    start: usize,
    prefix_length: usize,
) -> Result<(), Diagnostic> {
    cursor.advance_by(prefix_length);

    if !eat_digits(cursor, |b| matches!(b, b'0' | b'1')) {
        return Err(Diagnostic::e008(
            cursor.span_since(start),
            "binary literal requires at least one digit",
        ));
    }

    if is_touching_numeral(cursor) {
        return Err(touching_letter_error(cursor, start));
    }
    Ok(())
}

/// The cursor is past the first digit, or past a leading `.` when
/// `is_fraction` holds.
fn scan_decimal_number(
    cursor: &mut Cursor<'_>,
    start: usize,
    is_fraction: bool,
) -> Result<(), Diagnostic> {
    let mut has_point = is_fraction;
    if !is_fraction {
        eat_digits(cursor, u8::is_ascii_digit);
        if cursor.peek() == Some(b'.') && cursor.peek_next() != Some(b'.') {
            has_point = true;
            cursor.bump();
        }
    }
    if has_point {
        eat_digits(cursor, u8::is_ascii_digit);
    }

    if matches!(cursor.peek(), Some(b'e' | b'E')) {
        if !eat_exponent(cursor) {
            return Err(Diagnostic::e008(
                cursor.span_since(start),
                "decimal exponent requires digits",
            ));
        }
    } else {
        reject_adjacent_concat(cursor, start)?;
    }

    if is_touching_numeral(cursor) {
        return Err(touching_letter_error(cursor, start));
    }
    Ok(())
}

/// Whether the byte at the cursor would join the numeral in Luau's scanner
/// and leave it unconvertible.
fn is_touching_numeral(cursor: &Cursor<'_>) -> bool {
    cursor.peek().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn touching_letter_error(cursor: &Cursor<'_>, start: usize) -> Diagnostic {
    Diagnostic::e008(
        Span::from_indices(start, cursor.position() + 1),
        "malformed number (a numeral cannot touch a letter)",
    )
}

/// Consume digits that satisfy `is_digit` and `_` separators, and return
/// whether any digit was among them.
///
/// Luau strips every `_` before conversion, so `0b_01`, `1__2`, and `12_`
/// are all valid, with no placement rules.
fn eat_digits(cursor: &mut Cursor<'_>, is_digit: impl Fn(&u8) -> bool) -> bool {
    let digits_start = cursor.position();
    cursor.eat_while(|b| is_digit(&b) || b == b'_');
    cursor.text_since(digits_start).iter().any(|&b| b != b'_')
}

/// Consume an exponent with the cursor on its `e`: the marker, an optional
/// sign, and the decimal digits. Returns whether any digits followed.
fn eat_exponent(cursor: &mut Cursor<'_>) -> bool {
    cursor.bump();
    if matches!(cursor.peek(), Some(b'+' | b'-')) {
        cursor.bump();
    }
    eat_digits(cursor, u8::is_ascii_digit)
}

/// Luau's scanner takes every `.` after a numeral's digits, short of an
/// exponent, so `1..2` is "malformed number", never a concat (`1 ..2` needs
/// the space).
fn reject_adjacent_concat(cursor: &Cursor<'_>, start: usize) -> Result<(), Diagnostic> {
    if cursor.rest().starts_with(b"..") {
        return Err(Diagnostic::e008(
            Span::from_indices(start, cursor.position() + 2),
            "malformed number (a numeral cannot directly precede `..`)",
        ));
    }
    Ok(())
}
