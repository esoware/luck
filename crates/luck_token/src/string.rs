//! Working on raw token text instead of decoded values miscompiles: UTF-8
//! corruption, escape-state confusion, and `"\65"` comparing unequal to `"A"`.

use alloc::borrow::Cow;

use crate::{LuaVersion, is_lua_whitespace, strip_line_break};

/// The text between a quoted string literal's quotes, escapes left undecoded,
/// so `"\65"` and `"A"` give different bodies.
///
/// Returns `None` for a long-bracket string and for any text that isn't a
/// quoted literal.
#[must_use]
pub fn short_string_body(raw: &str) -> Option<&str> {
    let strip_quote = |quote: char| raw.strip_prefix(quote)?.strip_suffix(quote);
    strip_quote('"').or_else(|| strip_quote('\''))
}

/// Handles both quote forms with every Lua escape, and long-bracket strings
/// (EOL normalization, leading-newline strip, no escapes).
///
/// Returns `None` for any form this doesn't model exactly, including text that isn't a whole
/// string literal, and a caller that gets `None` must not fold. Lua 5.1 and
/// Luau read an undefined escape as the literal character, and Luau's long
/// strings keep a lone CR as content. The result borrows from `raw` when the body needs no
/// rewriting: a quoted string without escapes, or a long-bracket string
/// without a CR.
#[must_use]
pub fn decode_string(raw: &str, version: LuaVersion) -> Option<Cow<'_, [u8]>> {
    if raw.starts_with('[') {
        return decode_long_bracket(raw.as_bytes(), version);
    }

    let (&quote, after_quote) = raw.as_bytes().split_first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let (&closing_quote, inner) = after_quote.split_last()?;
    if closing_quote != quote {
        return None;
    }
    if !inner.contains(&b'\\') {
        return Some(Cow::Borrowed(inner));
    }
    let mut output = Vec::with_capacity(inner.len());
    let mut remaining = inner;
    loop {
        let run_length = run_before(remaining, |word| has_byte(word, b'\\'), |b| b == b'\\');
        let (run, rest) = remaining.split_at_checked(run_length).unwrap_or((remaining, &[]));
        output.extend_from_slice(run);
        let Some(escape) = rest.get(1..) else {
            break;
        };
        remaining = decode_escape(escape, version, &mut output)?;
    }
    Some(Cow::Owned(output))
}

/// How many bytes `bytes` starts with before the first that `is_wanted`
/// picks out, checking eight at a time while `word_has_wanted` finds none.
fn run_before(
    bytes: &[u8],
    word_has_wanted: impl Fn(u64) -> bool,
    is_wanted: impl Fn(u8) -> bool,
) -> usize {
    let mut index = 0usize;
    while let Some(&chunk) = bytes.get(index..).and_then(<[u8]>::first_chunk::<8>) {
        #[expect(clippy::little_endian_bytes, reason = "which byte matches doesn't count")]
        let word = u64::from_le_bytes(chunk);
        if word_has_wanted(word) {
            break;
        }
        index = index.saturating_add(8);
    }
    let rest = bytes.get(index..).unwrap_or_default();
    index.saturating_add(rest.iter().position(|&b| is_wanted(b)).unwrap_or(rest.len()))
}

const LOW_BITS: u64 = 0x0101_0101_0101_0101;
const HIGH_BITS: u64 = 0x8080_8080_8080_8080;

/// Whether one of the eight bytes in `word` is `byte`.
fn has_byte(word: u64, byte: u8) -> bool {
    has_byte_below(word ^ LOW_BITS.wrapping_mul(u64::from(byte)), 1)
}

/// Whether one of the eight bytes in `word` is below `limit`, which is at
/// most 128.
fn has_byte_below(word: u64, limit: u8) -> bool {
    word.wrapping_sub(LOW_BITS.wrapping_mul(u64::from(limit))) & !word & HIGH_BITS != 0
}

fn decode_long_bracket(raw: &[u8], version: LuaVersion) -> Option<Cow<'_, [u8]>> {
    let after_open = raw.strip_prefix(b"[")?;
    let level = after_open.iter().take_while(|&&b| b == b'=').count();
    let body_with_closer = after_open.get(level..)?.strip_prefix(b"[")?;
    let closer_length = level.saturating_add(2);
    let content_length = body_with_closer.len().checked_sub(closer_length)?;
    let (content, closer) = body_with_closer.split_at(content_length);
    let closer_equals = closer.strip_prefix(b"]")?.strip_suffix(b"]")?;
    if closer_equals.iter().any(|&b| b != b'=') {
        return None;
    }
    // Each line break becomes one newline, and one right after the opening
    // bracket is dropped (5.4 manual section 3.1). Every break but a lone LF
    // holds a CR, so a body without one is already the value.
    let body = strip_line_break(content, version).unwrap_or(content);
    if !body.contains(&b'\r') {
        return Some(Cow::Borrowed(body));
    }
    let mut output = Vec::with_capacity(body.len());
    let mut remaining = body;
    while let Some((&byte, rest)) = remaining.split_first() {
        let (decoded, after_decoded) = strip_line_break(remaining, version)
            .map_or((byte, rest), |after_break| (b'\n', after_break));
        output.push(decoded);
        remaining = after_decoded;
    }
    Some(Cow::Owned(output))
}

/// Decode the escape in `escape`, which starts just past its `\`, and return
/// the bytes after it.
fn decode_escape<'text>(
    escape: &'text [u8],
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> Option<&'text [u8]> {
    let (&escaped, rest) = escape.split_first()?;
    match escaped {
        b'a' => output.push(0x07),
        b'b' => output.push(0x08),
        b'f' => output.push(0x0C),
        b'n' => output.push(b'\n'),
        b'r' => output.push(b'\r'),
        b't' => output.push(b'\t'),
        b'v' => output.push(0x0B),
        b'\\' => output.push(b'\\'),
        b'"' => output.push(b'"'),
        b'\'' => output.push(b'\''),
        b'\n' => {
            output.push(b'\n');
            if version.has_lfcr_line_break() {
                return Some(rest.strip_prefix(b"\r").unwrap_or(rest));
            }
        }
        b'\r' => {
            output.push(b'\n');
            return Some(rest.strip_prefix(b"\n").unwrap_or(rest));
        }
        b'x' if version.has_hex_escape() => return decode_hex_escape(rest, output),
        b'z' if version.has_whitespace_escape() => {
            let whitespace_length = rest.iter().take_while(|&&b| is_lua_whitespace(b)).count();
            return rest.get(whitespace_length..);
        }
        b'0'..=b'9' => return decode_decimal_escape(escape, output),
        b'u' if version.has_unicode_escape() => {
            return decode_unicode_escape(rest, version, output);
        }
        // Lua 5.1 and Luau read any other escaped character as itself. Strict
        // versions reject the token, so refuse to fold it.
        _ if !version.has_strict_escapes() => output.push(escaped),
        _ => return None,
    }
    Some(rest)
}

fn decode_hex_escape<'text>(rest: &'text [u8], output: &mut Vec<u8>) -> Option<&'text [u8]> {
    let [high_digit, low_digit, after_digits @ ..] = rest else {
        return None;
    };
    let high_nibble = char::from(*high_digit).to_digit(16)?;
    let low_nibble = char::from(*low_digit).to_digit(16)?;
    output.push(u8::try_from((high_nibble << 4u32) | low_nibble).ok()?);
    Some(after_digits)
}

fn decode_decimal_escape<'text>(escape: &'text [u8], output: &mut Vec<u8>) -> Option<&'text [u8]> {
    let digit_count = escape.iter().take(3).take_while(|b| b.is_ascii_digit()).count();
    let (digits, rest) = escape.split_at(digit_count);
    let value = digits.iter().try_fold(0u8, |value, &digit| {
        value.checked_mul(10)?.checked_add(digit.wrapping_sub(b'0'))
    })?;
    output.push(value);
    Some(rest)
}

fn decode_unicode_escape<'text>(
    rest: &'text [u8],
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> Option<&'text [u8]> {
    let after_open = rest.strip_prefix(b"{")?;
    let digit_count = after_open.iter().take_while(|b| b.is_ascii_hexdigit()).count();
    let (hex_digits, after_digits) = after_open.split_at(digit_count);
    let remaining = after_digits.strip_prefix(b"}")?;
    let c = char::from_u32(unicode_escape_codepoint(hex_digits, version)?)?;
    let mut encoded = [0u8; 4];
    output.extend_from_slice(c.encode_utf8(&mut encoded).as_bytes());
    Some(remaining)
}

/// The code point the hex digits of a `\u{...}` escape spell under `version`,
/// or `None` when it's malformed there: no digits, too many, or too large a
/// value.
#[must_use]
pub fn unicode_escape_codepoint(hex_digits: &[u8], version: LuaVersion) -> Option<u32> {
    if hex_digits.is_empty() || (version.has_unicode_escape_digit_limit() && hex_digits.len() > 16)
    {
        return None;
    }
    let mut value: u32 = 0;
    for &digit in hex_digits {
        value = value.checked_mul(16)?.checked_add(char::from(digit).to_digit(16)?)?;
    }
    let limit = if version.has_extended_unicode_escape() { 0x7FFF_FFFF } else { 0x0010_FFFF };
    (value <= limit).then_some(value)
}

/// Append `bytes` to `output` as a double-quoted string literal, so a caller
/// can reuse one buffer across literals.
///
/// Escapes double quotes, backslashes, and control bytes. Valid UTF-8 runs
/// pass through intact, and bytes that aren't valid UTF-8 (such as those from
/// `\xFF` escapes) become decimal escapes.
pub fn encode_string(bytes: &[u8], output: &mut String) {
    output.reserve(bytes.len().saturating_add(2));
    output.push('"');
    if let Ok(text) = str::from_utf8(bytes) {
        push_escaped_text(output, text);
    } else {
        let mut remaining = bytes;
        for chunk in bytes.utf8_chunks() {
            push_escaped_text(output, chunk.valid());
            remaining = remaining.get(chunk.valid().len()..).unwrap_or_default();
            for &b in chunk.invalid() {
                remaining = remaining.get(1..).unwrap_or_default();
                let is_followed_by_digit = remaining.first().is_some_and(u8::is_ascii_digit);
                push_escape(output, b, is_followed_by_digit);
            }
        }
    }
    output.push('"');
}

/// Whether [`encode_string`] writes at least `length` bytes for `bytes`,
/// found without encoding them and from no more of them than it takes.
#[must_use]
pub fn encodes_to_at_least(bytes: &[u8], length: usize) -> bool {
    // Every byte takes one or more and the quotes two, and every byte the
    // encoder escapes takes one more.
    let Some(mut spare) = length.checked_sub(bytes.len().saturating_add(2)) else {
        return true;
    };
    let mut index = 0usize;
    while spare > 0 {
        let rest = bytes.get(index..).unwrap_or_default();
        index = index.saturating_add(run_before(rest, has_escaped_byte, is_escaped_byte));
        if index >= bytes.len() {
            return false;
        }
        spare = spare.saturating_sub(1);
        index = index.saturating_add(1);
    }
    true
}

/// The bytes that need escaping are all ASCII, so every split lands on a char boundary. `text`
/// ends at an invalid byte or the end of input, never a digit, so an escape
/// at its end needs no padding.
fn push_escaped_text(output: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let mut run_start = 0usize;
    loop {
        let rest = bytes.get(run_start..).unwrap_or_default();
        let index = run_start.saturating_add(run_before(rest, has_escaped_byte, is_escaped_byte));
        output.push_str(text.get(run_start..index).unwrap_or_default());
        let Some(&byte) = bytes.get(index) else {
            break;
        };
        run_start = index.saturating_add(1);
        let is_followed_by_digit = bytes.get(run_start).is_some_and(u8::is_ascii_digit);
        push_escape(output, byte, is_followed_by_digit);
    }
}

const fn is_escaped_byte(byte: u8) -> bool {
    matches!(byte, 0..=0x1F | 0x7F | b'"' | b'\\')
}

fn has_escaped_byte(word: u64) -> bool {
    has_byte_below(word, 0x20)
        || has_byte(word, 0x7F)
        || has_byte(word, b'"')
        || has_byte(word, b'\\')
}

fn push_escape(output: &mut String, byte: u8, is_followed_by_digit: bool) {
    let named = match byte {
        b'"' => "\\\"",
        b'\\' => "\\\\",
        b'\n' => "\\n",
        b'\r' => "\\r",
        0x07 => "\\a",
        0x08 => "\\b",
        0x0C => "\\f",
        b'\t' => "\\t",
        0x0B => "\\v",
        _ => {
            // Pad to 3 digits when a digit follows so the escape can't
            // absorb it (`\9` + `9` must not read as `\99`).
            output.push('\\');
            if is_followed_by_digit || byte >= 100 {
                output.push(char::from(b'0'.saturating_add(byte.div_euclid(100))));
            }
            if is_followed_by_digit || byte >= 10 {
                output.push(char::from(b'0'.saturating_add(byte.div_euclid(10).rem_euclid(10))));
            }
            output.push(char::from(b'0'.saturating_add(byte.rem_euclid(10))));
            return;
        }
    };
    output.push_str(named);
}
