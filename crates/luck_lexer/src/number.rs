use crate::LexError;
use crate::cursor::Cursor;
use luck_token::{LuaVersion, Span, TokenKind};

/// Lex a number literal. Cursor should be positioned at the first digit or `.` (when followed
/// by a digit). Handles all Lua version formats based on version flags.
pub fn lex_number(
    cursor: &mut Cursor,
    source: &str,
    version: LuaVersion,
) -> Result<TokenKind, LexError> {
    let start = cursor.position();

    if cursor.peek() == Some(b'0') && matches!(cursor.peek_at(1), Some(b'x' | b'X')) {
        lex_hex_number(cursor, source, version, start)
    } else if version.has_binary_literals()
        && cursor.peek() == Some(b'0')
        && matches!(cursor.peek_at(1), Some(b'b' | b'B'))
    {
        lex_binary_number(cursor, source, start)
    } else {
        lex_decimal_number(cursor, source, version, start)
    }
}

fn lex_hex_number(
    cursor: &mut Cursor,
    source: &str,
    version: LuaVersion,
    start: usize,
) -> Result<TokenKind, LexError> {
    cursor.advance(); // 0
    cursor.advance(); // x/X

    let allow_underscores = version.has_underscore_separators();
    let digit_start = cursor.position();
    eat_hex_digits(cursor, allow_underscores);
    let has_integer_part = cursor.position() > digit_start;

    if !has_integer_part
        && cursor.peek() != Some(b'.')
        && !matches!(cursor.peek(), Some(b'p' | b'P'))
    {
        return Err(crate::lex_error(
            Span::new(start as u32, cursor.position() as u32),
            "hex literal requires at least one digit after '0x'",
        ));
    }

    if version.has_hex_floats() {
        let has_dot = cursor.peek() == Some(b'.');
        if has_dot {
            cursor.advance(); // .
            let frac_start = cursor.position();
            eat_hex_digits(cursor, allow_underscores);
            let has_frac = cursor.position() > frac_start;

            if !has_integer_part && !has_frac {
                return Err(crate::lex_error(
                    Span::new(start as u32, cursor.position() as u32),
                    "hex float requires digits before or after decimal point",
                ));
            }

            // Hex floats require a p/P binary exponent per the Lua spec
            if matches!(cursor.peek(), Some(b'p' | b'P')) {
                cursor.advance();
                if matches!(cursor.peek(), Some(b'+' | b'-')) {
                    cursor.advance();
                }
                let exp_start = cursor.position();
                eat_decimal_digits(cursor, allow_underscores);
                if cursor.position() == exp_start {
                    return Err(crate::lex_error(
                        Span::new(start as u32, cursor.position() as u32),
                        "hex float exponent requires digits",
                    ));
                }
            } else {
                return Err(crate::lex_error(
                    Span::new(start as u32, cursor.position() as u32),
                    "hex float requires 'p' or 'P' exponent",
                ));
            }
        } else if matches!(cursor.peek(), Some(b'p' | b'P')) {
            cursor.advance();
            if matches!(cursor.peek(), Some(b'+' | b'-')) {
                cursor.advance();
            }
            let exp_start = cursor.position();
            eat_decimal_digits(cursor, allow_underscores);
            if cursor.position() == exp_start {
                return Err(crate::lex_error(
                    Span::new(start as u32, cursor.position() as u32),
                    "hex float exponent requires digits",
                ));
            }
        }
    } else if (cursor.peek() == Some(b'.')
        && cursor.peek_at(1).is_some_and(|b| b.is_ascii_hexdigit()))
        || matches!(cursor.peek(), Some(b'p' | b'P'))
    {
        return Err(crate::lex_error(
            Span::new(start as u32, cursor.position() as u32),
            "hex float literals are not supported in this Lua version",
        ));
    }

    let raw = &source[start..cursor.position()];
    validate_underscore_placement(raw, start, version)?;
    Ok(TokenKind::Number(raw.into()))
}

fn lex_binary_number(
    cursor: &mut Cursor,
    source: &str,
    start: usize,
) -> Result<TokenKind, LexError> {
    cursor.advance(); // 0
    cursor.advance(); // b/B

    let digit_start = cursor.position();
    cursor.eat_while(|b| b == b'0' || b == b'1' || b == b'_');

    if cursor.position() == digit_start {
        return Err(crate::lex_error(
            Span::new(start as u32, cursor.position() as u32),
            "binary literal requires at least one digit",
        ));
    }

    let raw = &source[start..cursor.position()];
    validate_underscore_placement(raw, start, LuaVersion::Luau)?;
    Ok(TokenKind::Number(raw.into()))
}

fn lex_decimal_number(
    cursor: &mut Cursor,
    source: &str,
    version: LuaVersion,
    start: usize,
) -> Result<TokenKind, LexError> {
    let allow_underscores = version.has_underscore_separators();

    eat_decimal_digits(cursor, allow_underscores);

    if cursor.peek() == Some(b'.') {
        let after_dot = cursor.peek_at(1);
        let consume_dot = match after_dot {
            Some(b) if b.is_ascii_digit() => true,
            Some(b'e' | b'E') => true,
            Some(b'.') => false, // `..` or `...`
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => false, // method access like `1.foo`
            _ => true,           // EOF, operators, whitespace - `1.` is a valid float
        };
        if consume_dot {
            cursor.advance(); // .
            eat_decimal_digits(cursor, allow_underscores);
        }
    }

    if matches!(cursor.peek(), Some(b'e' | b'E')) {
        cursor.advance();
        if matches!(cursor.peek(), Some(b'+' | b'-')) {
            cursor.advance();
        }
        let exp_start = cursor.position();
        eat_decimal_digits(cursor, allow_underscores);
        if cursor.position() == exp_start {
            return Err(crate::lex_error(
                Span::new(start as u32, cursor.position() as u32),
                "decimal exponent requires digits",
            ));
        }
    }

    let raw = &source[start..cursor.position()];
    validate_underscore_placement(raw, start, version)?;
    Ok(TokenKind::Number(raw.into()))
}

fn eat_hex_digits(cursor: &mut Cursor, allow_underscores: bool) {
    if allow_underscores {
        cursor.eat_while(|b| b.is_ascii_hexdigit() || b == b'_');
    } else {
        cursor.eat_while(|b| b.is_ascii_hexdigit());
    }
}

fn eat_decimal_digits(cursor: &mut Cursor, allow_underscores: bool) {
    if allow_underscores {
        cursor.eat_while(|b| b.is_ascii_digit() || b == b'_');
    } else {
        cursor.eat_while(|b| b.is_ascii_digit());
    }
}

/// Validate underscore placement in number literals. Underscores can appear between digits
/// but not at start/end of the digit sequence or adjacent to prefix (0x, 0b).
fn validate_underscore_placement(
    raw: &str,
    start: usize,
    version: LuaVersion,
) -> Result<(), LexError> {
    if !raw.contains('_') {
        return Ok(());
    }

    if !version.has_underscore_separators() {
        return Err(crate::lex_error(
            Span::new(start as u32, (start + raw.len()) as u32),
            "underscore separators in numbers are not supported in this Lua version",
        ));
    }

    let bytes = raw.as_bytes();

    let digit_start =
        if bytes.len() >= 2 && bytes[0] == b'0' && matches!(bytes[1], b'x' | b'X' | b'b' | b'B') {
            2
        } else {
            0
        };

    if digit_start < bytes.len() && bytes[digit_start] == b'_' {
        return Err(crate::lex_error(
            Span::new(start as u32, (start + raw.len()) as u32),
            "underscore cannot appear at start of number digits",
        ));
    }

    if bytes.last() == Some(&b'_') {
        return Err(crate::lex_error(
            Span::new(start as u32, (start + raw.len()) as u32),
            "underscore cannot appear at end of number",
        ));
    }

    for window in bytes.windows(2) {
        if window[0] == b'_' && window[1] == b'_' {
            return Err(crate::lex_error(
                Span::new(start as u32, (start + raw.len()) as u32),
                "consecutive underscores in number literal",
            ));
        }
    }

    Ok(())
}
