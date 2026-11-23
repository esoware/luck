//! What a string literal means, and the shortest literal for a value.
//!
//! A Lua string is bytes, which need not be UTF-8. One routine,
//! [`read_escape`], reads every escape sequence, for the lexer that checks
//! them and the parser that decodes them.

use core::iter;

use memchr::memmem;

use crate::{LuaVersion, is_lua_whitespace, strip_line_break};

/// What an escape sequence puts in a string's value.
///
/// That's no bytes for `\z`, one for most, and up to six for `\u{XXX}`,
/// which Lua writes as UTF-8, a surrogate or a value past U+10FFFF included,
/// in the original UTF-8's longer forms (`luaO_utf8esc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Escaped {
    bytes: [u8; 6],
    length: u8,
    is_lenient: bool,
}

impl Escaped {
    const fn byte(byte: u8) -> Self {
        Self { bytes: [byte, 0, 0, 0, 0, 0], length: 1, is_lenient: false }
    }

    const NOTHING: Self = Self { bytes: [0; 6], length: 0, is_lenient: false };

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.length)).unwrap_or_default()
    }

    /// Whether it escapes a character Lua 5.1 and Luau define no escape for,
    /// which they read as itself.
    #[must_use]
    pub const fn is_lenient(self) -> bool {
        self.is_lenient
    }
}

/// Why an escape sequence is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeError {
    /// `\ddd` past 255, with the value it spells.
    DecimalTooLarge(u16),
    /// `\x` without two hex digits, with how many came.
    HexDigits(u8),
    /// `\u` without its `{`.
    UnicodeBrace,
    /// `\u{` with no hex digit.
    UnicodeDigits,
    /// `\u{XXX}` past the code points the version reaches, or past Luau's 16
    /// digits.
    UnicodeTooLarge,
    /// `\u{XXX` without its `}`.
    UnicodeUnclosed,
    /// A character a strict dialect (5.2+) defines no escape for.
    Undefined,
}

/// Reads the escape sequence after a `\`, where `rest` starts.
///
/// Returns how many bytes of `rest` it spans, with what it puts in the
/// string or why it's malformed. A malformed one spans what its reader took
/// before giving up, so the string goes on after it. An interpolated string
/// (Luau) also defines `\{`, `\}`, and `` \` ``.
pub fn read_escape(
    rest: &[u8],
    is_interpolated: bool,
    version: LuaVersion,
) -> (usize, Result<Escaped, EscapeError>) {
    let Some((&escaped, after)) = rest.split_first() else {
        return (0, Err(EscapeError::Undefined));
    };
    let byte = |value: u8| (1, Ok(Escaped::byte(value)));
    match escaped {
        b'a' => byte(0x07),
        b'b' => byte(0x08),
        b'f' => byte(0x0C),
        b'n' => byte(b'\n'),
        b'r' => byte(b'\r'),
        b't' => byte(b'\t'),
        b'v' => byte(0x0B),
        b'\\' | b'"' | b'\'' => byte(escaped),
        b'\n' => {
            let is_lfcr = version.has_lfcr_line_break() && after.first() == Some(&b'\r');
            (if is_lfcr { 2 } else { 1 }, Ok(Escaped::byte(b'\n')))
        }
        b'\r' => (if after.first() == Some(&b'\n') { 2 } else { 1 }, Ok(Escaped::byte(b'\n'))),
        b'0'..=b'9' => {
            let digit_count = rest.iter().take(3).take_while(|b| b.is_ascii_digit()).count();
            let value = rest.iter().take(digit_count).fold(0u16, |value, &digit| {
                value.wrapping_mul(10).wrapping_add(u16::from(digit.wrapping_sub(b'0')))
            });
            let meaning = u8::try_from(value)
                .ok()
                .map_or(Err(EscapeError::DecimalTooLarge(value)), |decoded| {
                    Ok(Escaped::byte(decoded))
                });
            (digit_count, meaning)
        }
        b'x' if version.has_hex_escape() => {
            let digits = after.iter().take(2).take_while(|b| b.is_ascii_hexdigit());
            let (digit_count, value) = digits.fold((0u8, 0u8), |(count, value), &digit| {
                let nibble = char::from(digit).to_digit(16).and_then(|n| u8::try_from(n).ok());
                (count.saturating_add(1), value.wrapping_shl(4) | nibble.unwrap_or_default())
            });
            let length = usize::from(digit_count).saturating_add(1);
            if digit_count < 2 {
                return (length, Err(EscapeError::HexDigits(digit_count)));
            }
            (length, Ok(Escaped::byte(value)))
        }
        b'z' if version.has_whitespace_escape() => {
            let whitespace_length = after.iter().take_while(|&&b| is_lua_whitespace(b)).count();
            (whitespace_length.saturating_add(1), Ok(Escaped::NOTHING))
        }
        b'u' if version.has_unicode_escape() => read_unicode_escape(after, version),
        // Luau
        b'{' | b'}' | b'`' if is_interpolated => byte(escaped),
        _ if !version.has_strict_escapes() => {
            (1, Ok(Escaped { is_lenient: true, ..Escaped::byte(escaped) }))
        }
        _ => (1, Err(EscapeError::Undefined)),
    }
}

/// `after_u` starts past the `u` of `\u{XXX}`.
fn read_unicode_escape(
    after_u: &[u8],
    version: LuaVersion,
) -> (usize, Result<Escaped, EscapeError>) {
    let Some(after_open) = after_u.strip_prefix(b"{") else {
        return (1, Err(EscapeError::UnicodeBrace));
    };
    let digit_count = after_open.iter().take_while(|b| b.is_ascii_hexdigit()).count();
    let through_digits = digit_count.saturating_add(2);
    if digit_count == 0 {
        return (through_digits, Err(EscapeError::UnicodeDigits));
    }
    let digits = after_open.get(..digit_count).unwrap_or_default();
    let is_within_digit_limit = !version.has_unicode_escape_digit_limit() || digit_count <= 16;
    let spelled = digits.iter().try_fold(0u32, |value, &digit| {
        value.checked_mul(16)?.checked_add(char::from(digit).to_digit(16)?)
    });
    let limit = if version.has_extended_unicode_escape() { 0x7FFF_FFFF } else { 0x0010_FFFF };
    let Some(code_point) = spelled.filter(|&value| is_within_digit_limit && value <= limit) else {
        return (through_digits, Err(EscapeError::UnicodeTooLarge));
    };
    if after_open.get(digit_count) != Some(&b'}') {
        return (through_digits, Err(EscapeError::UnicodeUnclosed));
    }
    (through_digits.saturating_add(1), Ok(utf8_escape(code_point)))
}

/// Lua's `luaO_utf8esc` (`lobject.c`), which writes the continuation bytes
/// from the end and the lead byte last.
fn utf8_escape(code_point: u32) -> Escaped {
    if let Ok(ascii) = u8::try_from(code_point)
        && ascii.is_ascii()
    {
        return Escaped::byte(ascii);
    }
    let mut escaped = Escaped::NOTHING;
    let mut remaining = code_point;
    let mut lead_byte_room = 0x3Fu32;
    let mut written = 0usize;
    for slot in escaped.bytes.iter_mut().rev() {
        if remaining <= lead_byte_room {
            *slot =
                u8::try_from(((!lead_byte_room << 1u32) | remaining) & 0xFF).unwrap_or_default();
            written = written.saturating_add(1);
            break;
        }
        *slot = 0x80 | u8::try_from(remaining & 0x3F).unwrap_or_default();
        written = written.saturating_add(1);
        remaining >>= 6u32;
        lead_byte_room >>= 1u32;
    }
    escaped.bytes.rotate_right(written);
    escaped.length = u8::try_from(written).unwrap_or_default();
    escaped
}

/// Where a string literal's value ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringValue<'raw> {
    /// The value is these bytes of the literal's own spelling, which held
    /// nothing to rewrite.
    Verbatim(&'raw [u8]),
    /// The value had an escape or a line break to rewrite, and went to the
    /// buffer the decoder was given.
    Decoded,
}

/// The value of the string literal `raw`, as the lexer read it.
///
/// `raw` keeps its quotes or long brackets. A malformed escape, which the
/// lexer reports, puts nothing, and a literal cut off before its closer holds
/// what it reached. A long bracket's value drops a line break right after the
/// opener and turns every other one into a newline, but Luau keeps a lone CR.
pub fn decode_string<'raw>(
    raw: &'raw [u8],
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> StringValue<'raw> {
    match raw.first() {
        Some(&quote @ (b'"' | b'\'')) => {
            let after_quote = raw.get(1..).unwrap_or_default();
            let body = match after_quote.split_last() {
                Some((&last, before)) if last == quote => before,
                _ => after_quote,
            };
            decode_escapes(body, false, version, output)
        }
        Some(b'[') => decode_long_bracket(raw, version, output),
        _ => StringValue::Verbatim(raw),
    }
}

/// The value of a piece of a Luau interpolated string, the text between its
/// delimiters, as [`decode_string`] gives a quoted string's.
pub fn decode_interpolated_text<'raw>(
    text: &'raw [u8],
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> StringValue<'raw> {
    decode_escapes(text, true, version, output)
}

fn decode_escapes<'raw>(
    body: &'raw [u8],
    is_interpolated: bool,
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> StringValue<'raw> {
    let Some(first_escape) = memchr::memchr(b'\\', body) else {
        return StringValue::Verbatim(body);
    };
    output.reserve(body.len());
    let (before_escape, mut remaining) = body.split_at(first_escape);
    output.extend_from_slice(before_escape);
    while let Some(after_backslash) = remaining.get(1..) {
        let (length, meaning) = read_escape(after_backslash, is_interpolated, version);
        if let Ok(escaped) = meaning {
            output.extend_from_slice(escaped.as_bytes());
        }
        let rest = after_backslash.get(length..).unwrap_or_default();
        let run_length = memchr::memchr(b'\\', rest).unwrap_or(rest.len());
        let (run, after_run) = rest.split_at(run_length);
        output.extend_from_slice(run);
        if after_run.is_empty() {
            break;
        }
        remaining = after_run;
    }
    StringValue::Decoded
}

fn decode_long_bracket<'raw>(
    raw: &'raw [u8],
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> StringValue<'raw> {
    let after_open = raw.get(1..).unwrap_or_default();
    let level = after_open.iter().take_while(|&&b| b == b'=').count();
    let body_with_closer = after_open.get(level.saturating_add(1)..).unwrap_or_default();
    let closer_length = level.saturating_add(2);
    let content = body_with_closer
        .len()
        .checked_sub(closer_length)
        .and_then(|content_length| {
            let (content, closer) = body_with_closer.split_at(content_length);
            let closer_equals = closer.strip_prefix(b"]")?.strip_suffix(b"]")?;
            closer_equals.iter().all(|&b| b == b'=').then_some(content)
        })
        .unwrap_or(body_with_closer);
    // Each line break becomes one newline, and one right after the opening
    // bracket is dropped (5.4 manual section 3.1). Every break but a lone LF
    // holds a CR, so a body without one is already the value.
    let body = strip_line_break(content, version).unwrap_or(content);
    if memchr::memchr(b'\r', body).is_none() {
        return StringValue::Verbatim(body);
    }
    output.reserve(body.len());
    let mut remaining = body;
    while let Some((&byte, rest)) = remaining.split_first() {
        let (decoded, after_decoded) = strip_line_break(remaining, version)
            .map_or((byte, rest), |after_break| (b'\n', after_break));
        output.push(decoded);
        remaining = after_decoded;
    }
    StringValue::Decoded
}

/// The shortest string literal whose value is some bytes, in any version,
/// measured before it's written.
///
/// It's quoted with whichever quote needs fewer escapes, or in long brackets
/// when they save more than they cost. A quoted literal escapes its quote,
/// backslashes, control bytes, and bytes that aren't UTF-8, with decimal
/// escapes where nothing shorter is named, which no digit after them can
/// extend. Long brackets hold only UTF-8 with no control byte but newlines
/// and tabs. So the literal is UTF-8 whatever its value, and a printer that
/// spells values writes UTF-8 for a source that is.
#[derive(Debug, Clone, Copy)]
pub struct StringSpelling<'value> {
    bytes: &'value [u8],
    /// The level of the long brackets it takes, which hold only UTF-8, or
    /// `None` when it's quoted with `quote`.
    long_bracket_level: Option<usize>,
    quote: u8,
    length: usize,
}

impl StringSpelling<'_> {
    /// How many bytes the literal takes.
    #[must_use]
    pub const fn length(&self) -> usize {
        self.length
    }

    /// Whether it opens with a long bracket's `[`.
    #[must_use]
    pub const fn is_long_bracket(&self) -> bool {
        self.long_bracket_level.is_some()
    }

    /// Appends the literal to `output`.
    pub fn write_to(&self, output: &mut Vec<u8>) {
        match self.long_bracket_level {
            Some(level) => write_long_bracket(self.bytes, level, output),
            None => write_quoted(self.bytes, self.quote, output),
        }
    }
}

/// The shortest string literal whose value is `bytes`.
#[must_use]
pub fn spell_string(bytes: &[u8]) -> StringSpelling<'_> {
    let counts = EscapeCounts::of(bytes);
    let quote = if counts.double_quotes <= counts.single_quotes { b'"' } else { b'\'' };
    let quoted_length = bytes
        .len()
        .saturating_add(2)
        .saturating_add(counts.extra_bytes)
        .saturating_add(counts.double_quotes.min(counts.single_quotes));
    let leading_newline = usize::from(bytes.first() == Some(&b'\n'));
    if counts.can_be_long
        && quoted_length > bytes.len().saturating_add(4).saturating_add(leading_newline)
    {
        let level = long_bracket_level(bytes);
        let long_length = bytes
            .len()
            .saturating_add(level.saturating_mul(2))
            .saturating_add(4)
            .saturating_add(leading_newline);
        if long_length < quoted_length {
            let long_bracket_level = Some(level);
            return StringSpelling { bytes, long_bracket_level, quote, length: long_length };
        }
    }
    StringSpelling { bytes, long_bracket_level: None, quote, length: quoted_length }
}

/// Appends to `output` the long-bracket literal whose value is `text`.
///
/// It takes the lowest level every version reads back as `text`: one whose
/// closer neither appears in `text` nor finishes at its end, and above 0
/// when `text` holds a `[[`, which Lua 5.1 rejects at that level. Returns
/// false and appends nothing for text with a CR, which a long bracket can't
/// hold, or a NUL, which ends a Luau source.
pub fn write_long_string(text: &str, output: &mut Vec<u8>) -> bool {
    if text.contains(['\r', '\0']) {
        return false;
    }
    write_long_bracket(text.as_bytes(), long_bracket_level(text.as_bytes()), output);
    true
}

fn write_long_bracket(bytes: &[u8], level: usize, output: &mut Vec<u8>) {
    output.reserve(bytes.len().saturating_add(level.saturating_mul(2)).saturating_add(5));
    output.push(b'[');
    output.extend(iter::repeat_n(b'=', level));
    output.push(b'[');
    // Lua drops a line break right after the opener.
    if bytes.first() == Some(&b'\n') {
        output.push(b'\n');
    }
    output.extend_from_slice(bytes);
    output.push(b']');
    output.extend(iter::repeat_n(b'=', level));
    output.push(b']');
}

/// The lowest level a long bracket around `bytes` can take, as
/// [`write_long_string`] picks it.
fn long_bracket_level(bytes: &[u8]) -> usize {
    // Bit `n` says level `n` is taken, for the levels a word holds.
    let mut taken = u64::from(memmem::find(bytes, b"[[").is_some());
    let mut taken_beyond = Vec::new();
    for closer_start in memchr::memchr_iter(b']', bytes) {
        let after = bytes.get(closer_start.saturating_add(1)..).unwrap_or_default();
        let level = after.iter().take_while(|&&b| b == b'=').count();
        // A `]=*` at the end is finished by the literal's own closer.
        if matches!(after.get(level), Some(b']') | None) {
            match u32::try_from(level).ok().and_then(|bit| 1u64.checked_shl(bit)) {
                Some(bit) => taken |= bit,
                None => taken_beyond.push(level),
            }
        }
    }
    let lowest_free = usize::try_from(taken.trailing_ones()).unwrap_or_default();
    if lowest_free < 64 {
        return lowest_free;
    }
    taken_beyond.sort_unstable();
    taken_beyond.dedup();
    let mut level = 64usize;
    for taken_level in taken_beyond {
        if taken_level == level {
            level = level.saturating_add(1);
        }
    }
    level
}

/// What a quoted spelling of a string costs beyond its bytes and quotes.
#[derive(Default)]
struct EscapeCounts {
    double_quotes: usize,
    single_quotes: usize,
    /// The bytes escapes add, quotes aside.
    extra_bytes: usize,
    /// Whether long brackets can hold the bytes.
    can_be_long: bool,
}

impl EscapeCounts {
    fn of(bytes: &[u8]) -> Self {
        let mut counts = Self { can_be_long: true, ..Self::default() };
        for chunk in bytes.utf8_chunks() {
            let valid = chunk.valid().as_bytes();
            let mut index = 0usize;
            loop {
                let rest = valid.get(index..).unwrap_or_default();
                index = index.saturating_add(run_before(rest, has_quoted_byte, is_quoted_byte));
                let Some(&byte) = valid.get(index) else {
                    break;
                };
                index = index.saturating_add(1);
                match byte {
                    b'"' => counts.double_quotes = counts.double_quotes.saturating_add(1),
                    b'\'' => counts.single_quotes = counts.single_quotes.saturating_add(1),
                    _ => {
                        let is_followed_by_digit = valid.get(index).is_some_and(u8::is_ascii_digit);
                        let escape_length = escape(byte, is_followed_by_digit).len();
                        counts.extra_bytes =
                            counts.extra_bytes.saturating_add(escape_length.saturating_sub(1));
                        counts.can_be_long &= matches!(byte, b'\n' | b'\t' | b'\\');
                    }
                }
            }
            if !chunk.invalid().is_empty() {
                counts.can_be_long = false;
                // Every byte that isn't UTF-8 is past 127, a three-digit
                // escape.
                counts.extra_bytes =
                    counts.extra_bytes.saturating_add(chunk.invalid().len().saturating_mul(3));
            }
        }
        counts
    }
}

/// The escape a quoted spelling writes for `byte`, which it escapes.
fn escape(byte: u8, is_followed_by_digit: bool) -> EscapeText {
    let named = match byte {
        b'"' => "\\\"",
        b'\'' => "\\'",
        b'\\' => "\\\\",
        b'\n' => "\\n",
        b'\r' => "\\r",
        0x07 => "\\a",
        0x08 => "\\b",
        0x0C => "\\f",
        b'\t' => "\\t",
        0x0B => "\\v",
        // Luau: an interpolated string escapes its delimiters by name.
        b'{' => "\\{",
        b'`' => "\\`",
        _ => return EscapeText::decimal(byte, is_followed_by_digit),
    };
    EscapeText::named(named)
}

/// One escape, `\` included, on the stack.
struct EscapeText {
    bytes: [u8; 4],
    length: usize,
}

impl EscapeText {
    fn named(text: &str) -> Self {
        let mut bytes = [0u8; 4];
        let length = text.len().min(4);
        if let (Some(slot), Some(source)) = (bytes.get_mut(..length), text.as_bytes().get(..length))
        {
            slot.copy_from_slice(source);
        }
        Self { bytes, length }
    }

    /// Padded to three digits when a digit follows, so the escape can't
    /// absorb it (`\9` and `9` must not read as `\99`).
    fn decimal(byte: u8, is_followed_by_digit: bool) -> Self {
        let mut bytes = [b'\\', 0, 0, 0];
        let mut length = 1usize;
        let mut push = |digit: u8| {
            if let Some(slot) = bytes.get_mut(length) {
                *slot = b'0'.saturating_add(digit);
            }
            length = length.saturating_add(1);
        };
        if is_followed_by_digit || byte >= 100 {
            push(byte.div_euclid(100));
        }
        if is_followed_by_digit || byte >= 10 {
            push(byte.div_euclid(10).rem_euclid(10));
        }
        push(byte.rem_euclid(10));
        Self { bytes, length }
    }

    fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or_default()
    }

    const fn len(&self) -> usize {
        self.length
    }
}

/// Appends `bytes` quoted with `quote`, every byte that needs it escaped.
fn write_quoted(bytes: &[u8], quote: u8, output: &mut Vec<u8>) {
    output.reserve(bytes.len().saturating_add(2));
    output.push(quote);
    let needs_escape = |byte: u8| byte == quote || !matches!(byte, b'"' | b'\'');
    write_escaped(bytes, has_quoted_byte, is_quoted_byte, needs_escape, output);
    output.push(quote);
}

/// Appends `bytes` as the text of a Luau interpolated string's piece,
/// escaping backslashes, backticks, `{`, control bytes, and bytes that aren't
/// UTF-8.
pub fn write_interpolated_text(bytes: &[u8], output: &mut Vec<u8>) {
    write_escaped(bytes, has_interpolated_byte, is_interpolated_byte, |_| true, output);
}

/// Appends `bytes` with some of them escaped.
///
/// That's each byte that isn't UTF-8, and each that `is_candidate` picks out
/// and `needs_escape` keeps. `word_has_candidate` finds candidates eight at
/// a time.
fn write_escaped(
    bytes: &[u8],
    word_has_candidate: impl Fn(u64) -> bool,
    is_candidate: impl Fn(u8) -> bool,
    needs_escape: impl Fn(u8) -> bool,
    output: &mut Vec<u8>,
) {
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid().as_bytes();
        let mut run_start = 0usize;
        loop {
            let rest = valid.get(run_start..).unwrap_or_default();
            let index =
                run_start.saturating_add(run_before(rest, &word_has_candidate, &is_candidate));
            output.extend_from_slice(valid.get(run_start..index).unwrap_or_default());
            let Some(&byte) = valid.get(index) else {
                break;
            };
            run_start = index.saturating_add(1);
            if needs_escape(byte) {
                let is_followed_by_digit = valid.get(run_start).is_some_and(u8::is_ascii_digit);
                output.extend_from_slice(escape(byte, is_followed_by_digit).as_bytes());
            } else {
                output.push(byte);
            }
        }
        // Each byte that isn't UTF-8 is past 127, so its escape has three
        // digits already.
        for &byte in chunk.invalid() {
            output.extend_from_slice(escape(byte, false).as_bytes());
        }
    }
}

/// The bytes a quoted spelling may escape: control bytes, the quotes, and
/// backslashes.
const fn is_quoted_byte(byte: u8) -> bool {
    matches!(byte, 0..=0x1F | 0x7F | b'"' | b'\'' | b'\\')
}

fn has_quoted_byte(word: u64) -> bool {
    has_byte_below(word, 0x20)
        || has_byte(word, 0x7F)
        || has_byte(word, b'"')
        || has_byte(word, b'\'')
        || has_byte(word, b'\\')
}

/// The bytes an interpolated string's text escapes: control bytes,
/// backslashes, backticks, and `{`.
const fn is_interpolated_byte(byte: u8) -> bool {
    matches!(byte, 0..=0x1F | 0x7F | b'\\' | b'`' | b'{')
}

fn has_interpolated_byte(word: u64) -> bool {
    has_byte_below(word, 0x20)
        || has_byte(word, 0x7F)
        || has_byte(word, b'\\')
        || has_byte(word, b'`')
        || has_byte(word, b'{')
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
