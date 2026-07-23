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

    // Luau's numeral scanner consumes dots in hex literals too, so
    // `0xFF..2` is malformed there. 5.1's hex scanner stops at the dot
    // (valid concat) and 5.2+ already failed in the hex-float path.
    if version.is_luau() && cursor.peek() == Some(b'.') && cursor.peek_at(1) == Some(b'.') {
        return Err(crate::lex_error(
            Span::new(start as u32, (cursor.position() + 2) as u32),
            "malformed number (a numeral cannot directly precede '..')",
        ));
    }

    if version.has_luau_integer_literals() && cursor.peek() == Some(b'i') {
        cursor.advance();
        validate_integer_range(source, start, cursor.position(), 16)?;
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

    // Binary literals are Luau-only, and Luau's scanner consumes dots.
    if cursor.peek() == Some(b'.') && cursor.peek_at(1) == Some(b'.') {
        return Err(crate::lex_error(
            Span::new(start as u32, (cursor.position() + 2) as u32),
            "malformed number (a numeral cannot directly precede '..')",
        ));
    }

    if cursor.peek() == Some(b'i') {
        cursor.advance();
        validate_integer_range(source, start, cursor.position(), 2)?;
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

    let mut is_float = false;
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
            is_float = true;
            cursor.advance(); // .
            eat_decimal_digits(cursor, allow_underscores);
        }
    }

    if matches!(cursor.peek(), Some(b'e' | b'E')) {
        is_float = true;
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

    // Every real Lua numeral scanner consumes `.` greedily, so a number
    // running straight into `..` is "malformed number", never a concat;
    // `1 ..2` needs the space. (5.1's hex scanner stops at `.`, so hex
    // is exempt there - see lex_hex_number.)
    if cursor.peek() == Some(b'.') && cursor.peek_at(1) == Some(b'.') {
        return Err(crate::lex_error(
            Span::new(start as u32, (cursor.position() + 2) as u32),
            "malformed number (a numeral cannot directly precede '..')",
        ));
    }

    if version.has_luau_integer_literals() && cursor.peek() == Some(b'i') {
        cursor.advance();
        // Luau consumes the suffix on floats too: `2.5i` is a malformed
        // integer, not a float followed by an identifier.
        if is_float {
            return Err(crate::lex_error(
                Span::new(start as u32, cursor.position() as u32),
                "the 'i' integer suffix is not allowed on float literals",
            ));
        }
        validate_integer_range(source, start, cursor.position(), 10)?;
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

/// Reject underscore separators outside Luau. Luau itself strips
/// underscores anywhere in the literal before conversion, so `0b_01`,
/// `1__2`, and `12_` are all valid there - no placement rules.
fn validate_underscore_placement(
    raw: &str,
    start: usize,
    version: LuaVersion,
) -> Result<(), LexError> {
    if raw.contains('_') && !version.has_underscore_separators() {
        return Err(crate::lex_error(
            Span::new(start as u32, (start + raw.len()) as u32),
            "underscore separators in numbers are not supported in this Lua version",
        ));
    }
    Ok(())
}

/// Integer literals use signed decimal spelling but allow all 64 bits in
/// hexadecimal and binary spellings, matching Luau's bit-pattern semantics.
fn validate_integer_range(
    source: &str,
    start: usize,
    end: usize,
    radix: u32,
) -> Result<(), LexError> {
    let raw = &source[start..end - 1]; // exclude the trailing `i`
    let digits = match radix {
        16 | 2 => &raw[2..],
        10 => raw,
        _ => unreachable!("Luau integer literals use radix 2, 10, or 16"),
    };
    let normalized: String = digits.chars().filter(|ch| *ch != '_').collect();
    let valid = if radix == 10 {
        normalized.parse::<i64>().is_ok()
    } else {
        u64::from_str_radix(&normalized, radix).is_ok()
    };

    if valid {
        Ok(())
    } else {
        Err(crate::lex_error(
            Span::new(start as u32, end as u32),
            "integer literal is outside the 64-bit range",
        ))
    }
}
