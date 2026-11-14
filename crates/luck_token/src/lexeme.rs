//! Whether a text is one whole literal token, read as `luck_lexer` reads it.
//!
//! These follow the lexer's scanners, which its tests check against them, so
//! a crate that can't run the lexer can still tell whether a literal's text
//! reads back as that literal.

use memchr::memmem;

use crate::{LuaVersion, is_lua_whitespace, unicode_escape_codepoint};

/// Whether `text` is one numeral token in some dialect.
#[must_use]
pub fn is_numeral(text: &str) -> bool {
    LuaVersion::ALL.iter().any(|&version| is_numeral_in(text, version))
}

/// Whether `text` is one numeral token under `version`, a Luau `i` suffix
/// included.
#[must_use]
pub fn is_numeral_in(text: &str, version: LuaVersion) -> bool {
    let bytes = text.as_bytes();
    let rest = match bytes {
        [b'.', b'0', b'x' | b'X', ..] => None,
        [b'0', after_zero @ ..] => {
            // Luau strips every `_` before conversion, so `0_x1` is hex.
            let underscore_count = if version.has_underscore_separators() {
                after_zero.iter().take_while(|&&b| b == b'_').count()
            } else {
                0
            };
            match after_zero.get(underscore_count..) {
                Some([b'x' | b'X', digits @ ..]) => hex_numeral_rest(bytes, digits, version),
                // Luau
                Some([b'b' | b'B', digits @ ..]) if version.has_binary_literals() => {
                    binary_numeral_rest(bytes, digits, version)
                }
                _ => decimal_numeral_rest(bytes, version),
            }
        }
        [b'0'..=b'9', ..] | [b'.', b'0'..=b'9', ..] => decimal_numeral_rest(bytes, version),
        _ => None,
    };
    // A byte the lexer would reject after the numeral is left over here, so
    // its checks on what follows a numeral aren't repeated.
    rest.is_some_and(<[u8]>::is_empty)
}

/// What follows the run of digits `bytes` starts with, and whether the run
/// holds a digit. The run takes `_` where `version` allows it anywhere.
fn skip_digits(bytes: &[u8], version: LuaVersion, is_digit: fn(u8) -> bool) -> (&[u8], bool) {
    let is_in_run = |b: u8| is_digit(b) || (b == b'_' && version.has_underscore_separators());
    let run_length = bytes.iter().take_while(|&&b| is_in_run(b)).count();
    let (run, rest) = bytes.split_at(run_length);
    (rest, run.iter().any(|&b| b != b'_'))
}

/// What follows the exponent `bytes` starts with, its marker included, or
/// `None` when it has no digits.
fn skip_exponent(bytes: &[u8], version: LuaVersion) -> Option<&[u8]> {
    let after_marker = bytes.get(1..)?;
    let after_sign = after_marker
        .strip_prefix(b"+")
        .or_else(|| after_marker.strip_prefix(b"-"))
        .unwrap_or(after_marker);
    let (rest, has_digit) = skip_digits(after_sign, version, |b| b.is_ascii_digit());
    has_digit.then_some(rest)
}

fn hex_numeral_rest<'text>(
    numeral: &'text [u8],
    digits: &'text [u8],
    version: LuaVersion,
) -> Option<&'text [u8]> {
    let (mut rest, mut has_digit) = skip_digits(digits, version, |b| b.is_ascii_hexdigit());
    // Lua 5.2+
    if version.has_hex_floats() {
        if let Some(fraction) = rest.strip_prefix(b".") {
            let (after_fraction, has_fraction) =
                skip_digits(fraction, version, |b| b.is_ascii_hexdigit());
            has_digit |= has_fraction;
            rest = after_fraction;
        }
        if has_digit && matches!(rest.first(), Some(b'p' | b'P')) {
            rest = skip_exponent(rest, version)?;
        }
    }
    if !has_digit {
        return None;
    }
    integer_suffix_rest(numeral, rest, version, 16)
}

fn binary_numeral_rest<'text>(
    numeral: &'text [u8],
    digits: &'text [u8],
    version: LuaVersion,
) -> Option<&'text [u8]> {
    let (rest, has_digit) = skip_digits(digits, version, |b| matches!(b, b'0' | b'1'));
    if !has_digit {
        return None;
    }
    integer_suffix_rest(numeral, rest, version, 2)
}

fn decimal_numeral_rest(numeral: &[u8], version: LuaVersion) -> Option<&[u8]> {
    let (mut rest, _) = skip_digits(numeral, version, |b| b.is_ascii_digit());
    let mut is_float = false;
    if let Some(fraction) = rest.strip_prefix(b".").filter(|fraction| !fraction.starts_with(b".")) {
        is_float = true;
        rest = skip_digits(fraction, version, |b| b.is_ascii_digit()).0;
    }
    if matches!(rest.first(), Some(b'e' | b'E')) {
        is_float = true;
        rest = skip_exponent(rest, version)?;
    }
    // A float takes no `i` suffix, so one stays behind.
    if is_float { Some(rest) } else { integer_suffix_rest(numeral, rest, version, 10) }
}

/// What follows a Luau `i` suffix at the start of `rest`, and the `_` after
/// it, or `rest` itself when there's no suffix. `None` when the integer
/// doesn't fit in 64 bits.
fn integer_suffix_rest<'text>(
    numeral: &'text [u8],
    rest: &'text [u8],
    version: LuaVersion,
    radix: u32,
) -> Option<&'text [u8]> {
    let Some(after_suffix) =
        rest.strip_prefix(b"i").filter(|_| version.has_luau_integer_literals())
    else {
        return Some(rest);
    };
    let spelled = numeral.get(..numeral.len().saturating_sub(rest.len()))?;
    if !is_in_integer_range(spelled, radix) {
        return None;
    }
    let underscore_count = after_suffix.iter().take_while(|&&b| b == b'_').count();
    after_suffix.get(underscore_count..)
}

/// A decimal integer must fit an `i64`, while hexadecimal and binary ones
/// may use all 64 bits, as Luau reads them.
fn is_in_integer_range(spelled: &[u8], radix: u32) -> bool {
    let prefix_length = if radix == 10 { 0 } else { 2 };
    let mut value = 0u64;
    let mut has_digit = false;
    for &b in spelled.iter().filter(|&&b| b != b'_').skip(prefix_length) {
        let next = char::from(b)
            .to_digit(radix)
            .and_then(|digit| value.checked_mul(u64::from(radix))?.checked_add(u64::from(digit)));
        let Some(next_value) = next else {
            return false;
        };
        value = next_value;
        has_digit = true;
    }
    has_digit && (radix != 10 || i64::try_from(value).is_ok())
}

/// Whether `text` is one string literal token in some dialect, quoted or
/// long-bracket.
#[must_use]
pub fn is_string_literal(text: &str) -> bool {
    LuaVersion::ALL.iter().any(|&version| is_string_literal_in(text, version))
}

#[must_use]
pub fn is_string_literal_in(text: &str, version: LuaVersion) -> bool {
    let bytes = text.as_bytes();
    if is_cut_short(bytes, version) {
        return false;
    }
    match bytes {
        [quote @ (b'"' | b'\''), body @ ..] => is_short_string_body(body, *quote, version),
        [b'[', after_bracket @ ..] => is_long_string_rest(after_bracket, version),
        _ => false,
    }
}

/// Whether `version`'s lexer would stop reading before the end of `bytes`.
fn is_cut_short(bytes: &[u8], version: LuaVersion) -> bool {
    version.has_nul_source_end() && memchr::memchr(0, bytes).is_some()
}

/// `body` follows the opening quote.
fn is_short_string_body(body: &[u8], quote: u8, version: LuaVersion) -> bool {
    let mut rest = body;
    loop {
        match rest {
            [] | [b'\n' | b'\r', ..] => return false,
            [b'\\', escape @ ..] => match skip_escape(escape, version) {
                Some(after_escape) => rest = after_escape,
                None => return false,
            },
            [b, after_quote @ ..] if *b == quote => return after_quote.is_empty(),
            [_, ..] => {
                let stop = memchr::memchr3(quote, b'\\', b'\n', rest).unwrap_or(rest.len());
                let (run, after_run) = rest.split_at(stop);
                if memchr::memchr(b'\r', run).is_some() {
                    return false;
                }
                rest = after_run;
            }
        }
    }
}

/// What follows the escape `escape` starts with, just past its `\`, or
/// `None` when `version` rejects it.
fn skip_escape(escape: &[u8], version: LuaVersion) -> Option<&[u8]> {
    let (&escaped, rest) = escape.split_first()?;
    match escaped {
        b'\n' => Some(if version.has_lfcr_line_break() {
            rest.strip_prefix(b"\r").unwrap_or(rest)
        } else {
            rest
        }),
        b'\r' => Some(rest.strip_prefix(b"\n").unwrap_or(rest)),
        b'0'..=b'9' => {
            let digit_count = escape.iter().take(3).take_while(|b| b.is_ascii_digit()).count();
            let (digits, after_digits) = escape.split_at(digit_count);
            let value: u16 = str::from_utf8(digits).ok()?.parse().ok()?;
            (value <= 255).then_some(after_digits)
        }
        b'x' if version.has_hex_escape() => match rest {
            [high, low, after_digits @ ..]
                if high.is_ascii_hexdigit() && low.is_ascii_hexdigit() =>
            {
                Some(after_digits)
            }
            _ => None,
        },
        b'z' if version.has_whitespace_escape() => {
            let whitespace_length = rest.iter().take_while(|&&b| is_lua_whitespace(b)).count();
            rest.get(whitespace_length..)
        }
        b'u' if version.has_unicode_escape() => {
            let after_open = rest.strip_prefix(b"{")?;
            let digit_count = after_open.iter().take_while(|b| b.is_ascii_hexdigit()).count();
            let (hex_digits, after_digits) = after_open.split_at(digit_count);
            unicode_escape_codepoint(hex_digits, version)?;
            after_digits.strip_prefix(b"}")
        }
        b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'\'' | b'"' => Some(rest),
        // Lua 5.1 and Luau read any other escaped character as itself.
        _ if !version.has_strict_escapes() => Some(rest),
        _ => None,
    }
}

/// `after_bracket` follows the opening `[`. Only a closer at the very end of
/// the text ends the token there.
fn is_long_string_rest(after_bracket: &[u8], version: LuaVersion) -> bool {
    let level = after_bracket.iter().take_while(|&&b| b == b'=').count();
    let Some(body) = after_bracket.get(level..).and_then(|rest| rest.strip_prefix(b"[")) else {
        return false;
    };
    let mut searched = 0usize;
    while let Some(offset) = body.get(searched..).and_then(|rest| memchr::memchr(b']', rest)) {
        let after_close_bracket = searched.saturating_add(offset).saturating_add(1);
        let rest = body.get(after_close_bracket..).unwrap_or_default();
        let closing_level = rest.iter().take_while(|&&b| b == b'=').count();
        if closing_level == level && rest.get(level) == Some(&b']') {
            let end = after_close_bracket.saturating_add(level).saturating_add(1);
            let is_nesting_refused = level == 0
                && version.has_long_bracket_nesting_error()
                && memmem::find(body.get(..end).unwrap_or_default(), b"[[").is_some();
            return end == body.len() && !is_nesting_refused;
        }
        searched = after_close_bracket;
    }
    false
}

/// Whether `text` is the raw text of one interpolated string segment, escapes
/// as written, in some dialect that has them.
#[must_use]
pub fn is_interpolated_text(text: &str) -> bool {
    LuaVersion::ALL.iter().any(|&version| {
        version.has_interpolated_strings() && is_interpolated_text_in(text, version)
    })
}

fn is_interpolated_text_in(text: &str, version: LuaVersion) -> bool {
    let bytes = text.as_bytes();
    if is_cut_short(bytes, version) {
        return false;
    }
    let mut rest = bytes;
    loop {
        match rest {
            [] => return true,
            // `{` opens an expression and a backtick closes the string, so
            // either one ends the segment early.
            [b'\n' | b'\r' | b'{' | b'`', ..] => return false,
            [b'\\', b'{' | b'}' | b'`', after_escape @ ..] => rest = after_escape,
            [b'\\', escape @ ..] => match skip_escape(escape, version) {
                Some(after_escape) => rest = after_escape,
                None => return false,
            },
            [_, after @ ..] => rest = after,
        }
    }
}
