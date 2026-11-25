//! What a string literal means, and the shortest literal for a value.
//!
//! A Lua string is bytes, which need not be UTF-8. One routine,
//! [`read_escape`], reads every escape sequence, for the lexer that checks
//! them and the parser that decodes them.

use std::iter;

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

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// Whether it escapes a character Lua 5.1 and Luau define no escape for,
    /// which they read as itself.
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

/// The kind of string an escape is in, which decides the escapes it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringKind {
    /// In single or double quotes.
    Quoted,
    /// Luau: the text of an interpolated string, in backticks.
    Interpolated,
}

/// Reads the escape sequence after a `\`, where `rest` starts.
///
/// Returns how many bytes of `rest` it spans, with what it puts in the
/// string or why it's malformed. A malformed one spans what its reader took
/// before giving up, so the string goes on after it. An interpolated string
/// (Luau) also defines `\{`, `\}`, and `` \` ``.
#[expect(clippy::missing_panics_doc, reason = "`\\x` reads only hex digits, and two fit a byte")]
pub fn read_escape(
    rest: &[u8],
    kind: StringKind,
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
            let value = rest
                .iter()
                .take(digit_count)
                .fold(0, |value, &digit| value * 10 + u16::from(digit - b'0'));
            let meaning = u8::try_from(value)
                .map(Escaped::byte)
                .map_err(|_| EscapeError::DecimalTooLarge(value));
            (digit_count, meaning)
        }
        b'x' if version.has_hex_escape() => {
            let digits = after.iter().take(2).take_while(|b| b.is_ascii_hexdigit());
            let (digit_count, value) = digits.fold((0, 0), |(count, value), &digit| {
                let nibble = char::from(digit).to_digit(16).expect("a hex digit");
                (count + 1, (value << 4) | nibble)
            });
            let length = usize::from(digit_count) + 1;
            if digit_count < 2 {
                return (length, Err(EscapeError::HexDigits(digit_count)));
            }
            (length, Ok(Escaped::byte(u8::try_from(value).expect("two hex digits fit a byte"))))
        }
        b'z' if version.has_whitespace_escape() => {
            let whitespace_length = after.iter().take_while(|&&b| is_lua_whitespace(b)).count();
            (whitespace_length + 1, Ok(Escaped::NOTHING))
        }
        b'u' if version.has_unicode_escape() => read_unicode_escape(after, version),
        // Luau
        b'{' | b'}' | b'`' if kind == StringKind::Interpolated => byte(escaped),
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
    let through_digits = digit_count + 2;
    if digit_count == 0 {
        return (through_digits, Err(EscapeError::UnicodeDigits));
    }
    let digits = &after_open[..digit_count];
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
    (through_digits + 1, Ok(utf8_escape(code_point)))
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
    let mut lead_byte_room = 0x3F;
    let mut written = 0;
    for slot in escaped.bytes.iter_mut().rev() {
        written += 1;
        if remaining <= lead_byte_room {
            *slot = (((!lead_byte_room << 1) | remaining) & 0xFF) as u8;
            break;
        }
        *slot = 0x80 | (remaining & 0x3F) as u8;
        remaining >>= 6;
        lead_byte_room >>= 1;
    }
    escaped.bytes.rotate_right(written);
    escaped.length = u8::try_from(written).expect("at most six bytes");
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
///
/// `has_escape` is whether `raw` holds an escape, or in long brackets a CR,
/// as its token says, without which it holds nothing to rewrite. True makes
/// the decoder look.
pub fn decode_string<'raw>(
    raw: &'raw [u8],
    has_escape: bool,
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> StringValue<'raw> {
    match raw.first() {
        Some(&quote @ (b'"' | b'\'')) => {
            let after_quote = &raw[1..];
            let body = match after_quote.split_last() {
                Some((&last, before)) if last == quote => before,
                _ => after_quote,
            };
            if has_escape {
                decode_escapes(body, StringKind::Quoted, version, output)
            } else {
                StringValue::Verbatim(body)
            }
        }
        Some(b'[') => decode_long_bracket(raw, has_escape, version, output),
        _ => StringValue::Verbatim(raw),
    }
}

/// The value of a piece of a Luau interpolated string, the text between its
/// delimiters, as [`decode_string`] gives a quoted string's.
pub fn decode_interpolated_text<'raw>(
    text: &'raw [u8],
    has_escape: bool,
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> StringValue<'raw> {
    if has_escape {
        decode_escapes(text, StringKind::Interpolated, version, output)
    } else {
        StringValue::Verbatim(text)
    }
}

fn decode_escapes<'raw>(
    body: &'raw [u8],
    kind: StringKind,
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> StringValue<'raw> {
    let Some(first_escape) = memchr::memchr(b'\\', body) else {
        return StringValue::Verbatim(body);
    };
    output.reserve(body.len());
    let (before_escape, mut remaining) = body.split_at(first_escape);
    output.extend_from_slice(before_escape);
    loop {
        let after_backslash = &remaining[1..];
        let (length, meaning) = read_escape(after_backslash, kind, version);
        if let Ok(escaped) = meaning {
            output.extend_from_slice(escaped.as_bytes());
        }
        let rest = &after_backslash[length..];
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
    has_cr: bool,
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> StringValue<'raw> {
    let after_open = &raw[1..];
    let level = after_open.iter().take_while(|&&b| b == b'=').count();
    let body_with_closer = &after_open[level + 1..];
    let closer_length = level + 2;
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
    if !has_cr || memchr::memchr(b'\r', body).is_none() {
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
    pub const fn length(&self) -> usize {
        self.length
    }

    /// Whether it opens with a long bracket's `[`.
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
pub fn spell_string(bytes: &[u8]) -> StringSpelling<'_> {
    spelling(bytes, &EscapeCounts::of(bytes, usize::MAX))
}

/// [`spell_string`], when that's shorter than `length` bytes.
///
/// It stops measuring once no spelling can be, so a literal no spelling
/// beats costs a glance at its first few escapes, however long it is.
pub fn spell_string_shorter_than(bytes: &[u8], length: usize) -> Option<StringSpelling<'_>> {
    Some(spelling(bytes, &EscapeCounts::of(bytes, length)))
        .filter(|spelling| spelling.length < length)
}

fn spelling<'value>(bytes: &'value [u8], counts: &EscapeCounts) -> StringSpelling<'value> {
    let quote = if counts.double_quotes <= counts.single_quotes { b'"' } else { b'\'' };
    let quoted_length = counts.quoted_length(bytes.len());
    let leading_newline = usize::from(bytes.first() == Some(&b'\n'));
    if counts.can_be_long && quoted_length > bytes.len() + 4 + leading_newline {
        let level = long_bracket_level(bytes);
        let long_length = bytes.len() + level * 2 + 4 + leading_newline;
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
    output.reserve(bytes.len() + level * 2 + 5);
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
        let after = &bytes[closer_start + 1..];
        let level = after.iter().take_while(|&&b| b == b'=').count();
        // A `]=*` at the end is finished by the literal's own closer.
        if matches!(after.get(level), Some(b']') | None) {
            if level < 64 {
                taken |= 1 << level;
            } else {
                taken_beyond.push(level);
            }
        }
    }
    let lowest_free = taken.trailing_ones() as usize;
    if lowest_free < 64 {
        return lowest_free;
    }
    taken_beyond.sort_unstable();
    taken_beyond.dedup();
    let mut level = 64;
    for taken_level in taken_beyond {
        if taken_level == level {
            level += 1;
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
    /// Counts until a quoted spelling would take `limit` bytes or more and
    /// long brackets couldn't take fewer, past which no count matters, and
    /// then says long brackets can't hold `bytes`.
    fn of(bytes: &[u8], limit: usize) -> Self {
        let mut counts = Self { can_be_long: true, ..Self::default() };
        let leading_newline = usize::from(bytes.first() == Some(&b'\n'));
        let is_long_too_long = bytes.len() + 4 + leading_newline >= limit;
        let is_settled = |so_far: &Self| {
            so_far.quoted_length(bytes.len()) >= limit && (is_long_too_long || !so_far.can_be_long)
        };
        for chunk in bytes.utf8_chunks() {
            let valid = chunk.valid().as_bytes();
            let mut index = 0;
            loop {
                index += run_before(&valid[index..], quoted_bytes, is_quoted_byte);
                let Some(&byte) = valid.get(index) else {
                    break;
                };
                index += 1;
                match byte {
                    b'"' => counts.double_quotes += 1,
                    b'\'' => counts.single_quotes += 1,
                    _ => {
                        let is_followed_by_digit = valid.get(index).is_some_and(u8::is_ascii_digit);
                        counts.extra_bytes += escape(byte, is_followed_by_digit).length - 1;
                        counts.can_be_long &= matches!(byte, b'\n' | b'\t' | b'\\');
                    }
                }
                if is_settled(&counts) {
                    counts.can_be_long = false;
                    return counts;
                }
            }
            if !chunk.invalid().is_empty() {
                counts.can_be_long = false;
                // Every byte that isn't UTF-8 is past 127, a three-digit
                // escape.
                counts.extra_bytes += chunk.invalid().len() * 3;
                if is_settled(&counts) {
                    return counts;
                }
            }
        }
        counts
    }

    /// How many bytes a value of `value_length` bytes takes quoted.
    fn quoted_length(&self, value_length: usize) -> usize {
        value_length + 2 + self.extra_bytes + self.double_quotes.min(self.single_quotes)
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
        let mut bytes = [0; 4];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Self { bytes, length: text.len() }
    }

    /// Padded to three digits when a digit follows, so the escape can't
    /// absorb it (`\9` and `9` must not read as `\99`).
    fn decimal(byte: u8, is_followed_by_digit: bool) -> Self {
        let mut bytes = [b'\\', 0, 0, 0];
        let mut length = 1;
        let mut push = |digit: u8| {
            bytes[length] = b'0' + digit;
            length += 1;
        };
        if is_followed_by_digit || byte >= 100 {
            push(byte / 100);
        }
        if is_followed_by_digit || byte >= 10 {
            push(byte / 10 % 10);
        }
        push(byte % 10);
        Self { bytes, length }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

/// Appends `bytes` quoted with `quote`, every byte that needs it escaped.
fn write_quoted(bytes: &[u8], quote: u8, output: &mut Vec<u8>) {
    output.reserve(bytes.len() + 2);
    output.push(quote);
    let needs_escape = |byte: u8| byte == quote || !matches!(byte, b'"' | b'\'');
    write_escaped(bytes, quoted_bytes, is_quoted_byte, needs_escape, output);
    output.push(quote);
}

/// Appends `bytes` as the text of a Luau interpolated string's piece,
/// escaping backslashes, backticks, `{`, control bytes, and bytes that aren't
/// UTF-8.
pub fn write_interpolated_text(bytes: &[u8], output: &mut Vec<u8>) {
    write_escaped(bytes, interpolated_bytes, is_interpolated_byte, |_| true, output);
}

/// Appends `bytes` with some of them escaped.
///
/// That's each byte that isn't UTF-8, and each that `is_candidate` picks out
/// and `needs_escape` keeps. `candidate_bytes` finds candidates eight at a
/// time, as [`run_before`] reads them.
fn write_escaped(
    bytes: &[u8],
    candidate_bytes: impl Fn(u64) -> u64,
    is_candidate: impl Fn(u8) -> bool,
    needs_escape: impl Fn(u8) -> bool,
    output: &mut Vec<u8>,
) {
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid().as_bytes();
        let mut run_start = 0;
        loop {
            let index =
                run_start + run_before(&valid[run_start..], &candidate_bytes, &is_candidate);
            output.extend_from_slice(&valid[run_start..index]);
            let Some(&byte) = valid.get(index) else {
                break;
            };
            run_start = index + 1;
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

fn quoted_bytes(word: u64) -> u64 {
    let flagged = word.wrapping_sub(splat(0x20))
        | (word ^ splat(0x7F)).wrapping_sub(LOW_BITS)
        | (word ^ splat(b'"')).wrapping_sub(LOW_BITS)
        | (word ^ splat(b'\'')).wrapping_sub(LOW_BITS)
        | (word ^ splat(b'\\')).wrapping_sub(LOW_BITS);
    flagged & !word & HIGH_BITS
}

/// The bytes an interpolated string's text escapes: control bytes,
/// backslashes, backticks, and `{`.
const fn is_interpolated_byte(byte: u8) -> bool {
    matches!(byte, 0..=0x1F | 0x7F | b'\\' | b'`' | b'{')
}

fn interpolated_bytes(word: u64) -> u64 {
    let flagged = word.wrapping_sub(splat(0x20))
        | (word ^ splat(0x7F)).wrapping_sub(LOW_BITS)
        | (word ^ splat(b'\\')).wrapping_sub(LOW_BITS)
        | (word ^ splat(b'`')).wrapping_sub(LOW_BITS)
        | (word ^ splat(b'{')).wrapping_sub(LOW_BITS);
    flagged & !word & HIGH_BITS
}

/// How many bytes `bytes` starts with before the first that `is_wanted`
/// picks out, checking eight at a time with `wanted_bytes`.
///
/// `wanted_bytes` answers for each ASCII byte of a word in its high bit, by
/// subtractions that each set it for a byte below a limit, or for one that
/// is zero once XOR-ed with the byte it looks for. A subtraction borrows
/// across bytes only out of one that answers, so the lowest bit set marks
/// the first wanted byte, though those above it can be wrong.
fn run_before(
    bytes: &[u8],
    wanted_bytes: impl Fn(u64) -> u64,
    is_wanted: impl Fn(u8) -> bool,
) -> usize {
    let mut index = 0;
    while let Some(&chunk) = bytes[index..].first_chunk::<8>() {
        let wanted = wanted_bytes(u64::from_le_bytes(chunk));
        if wanted != 0 {
            return index + (wanted.trailing_zeros() / 8) as usize;
        }
        index += 8;
    }
    let rest = &bytes[index..];
    index + rest.iter().position(|&b| is_wanted(b)).unwrap_or(rest.len())
}

const LOW_BITS: u64 = 0x0101_0101_0101_0101;
const HIGH_BITS: u64 = 0x8080_8080_8080_8080;

fn splat(byte: u8) -> u64 {
    LOW_BITS * u64::from(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A borrow between bytes could flag a byte for its neighbor below, so
    /// every pair of bytes is tried side by side, at each place in a word.
    #[test]
    fn word_scans_find_the_first_wanted_byte_whatever_its_neighbor() {
        for low in 0..=u8::MAX {
            for high in 0..=u8::MAX {
                for position in 0..8 {
                    let mut bytes = [b'a'; 9];
                    bytes[position] = low;
                    bytes[position + 1] = high;
                    let first = |is_wanted: fn(u8) -> bool| {
                        bytes.iter().position(|&b| is_wanted(b)).unwrap_or(bytes.len())
                    };
                    assert_eq!(
                        run_before(&bytes, quoted_bytes, is_quoted_byte),
                        first(is_quoted_byte),
                        "{bytes:?}"
                    );
                    assert_eq!(
                        run_before(&bytes, interpolated_bytes, is_interpolated_byte),
                        first(is_interpolated_byte),
                        "{bytes:?}"
                    );
                }
            }
        }
    }
}
