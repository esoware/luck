//! Validators for the string DSLs of Lua's stdlib.
//!
//! They cover `string.format` specifiers, Lua patterns
//! (`string.match`/`gmatch`/`find`/`gsub`), `string.pack`/`string.unpack`
//! format strings, `gsub` replacement strings, and `os.date` formats.
//!
//! Each validator takes a literal's body without its quotes and returns a
//! count (specifiers, captures, or values consumed) where one applies, or
//! the first [`PatternError`] it finds. Callers map the error's offset back
//! into source.

use core::error::Error;
use core::fmt;
use core::iter::from_fn;

/// The first malformed construct a validator finds. Each `offset` is a
/// byte index into the body the validator was given.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub(crate) enum PatternError {
    /// A `%` construct cut off by the end of the body.
    TruncatedSpecifier { offset: usize },
    /// A `string.format` conversion character Lua doesn't know.
    UnknownSpecifier { offset: usize, character: char },
    /// A flag repeated within one `string.format` specifier.
    BadFlag { offset: usize, character: char },
    /// A width longer than the two digits Lua allows.
    BadWidth { offset: usize },
    /// A precision longer than the two digits Lua allows.
    BadPrecision { offset: usize },
    /// A `[set]` with no closing `]`.
    UnterminatedSet { offset: usize },
    /// A set with `]` right after its `[` or `[^`, as in `[]`.
    EmptySet { offset: usize },
    /// A `%` before a letter that isn't a pattern class or item, or a `%f`
    /// with no `[set]` after it.
    BadEscape { offset: usize, character: char },
    /// A `*`, `+`, `-`, or `?` with nothing before it to repeat, such as at
    /// the start of a pattern or right after `(`.
    InvalidQuantifierTarget { offset: usize },
    /// A `)` with no open capture, or a `(` that never closes.
    UnmatchedCapture { offset: usize },
    /// An unknown `string.pack` option, or an option `X` can't align to.
    InvalidPackOption { offset: usize, character: char },
    /// A `c` with no size, or an `X` with no option after it.
    TruncatedPackSize { offset: usize },
    /// A `gsub` replacement or `os.date` format ending in a single `%`.
    LonePercent { offset: usize },
    /// A `gsub` replacement `%` before something other than a digit or `%`.
    BadReplacementEscape { offset: usize },
    /// A `gsub` replacement `%n` above the pattern's capture count.
    CaptureIndexOutOfRange { offset: usize },
    /// An `os.date` format holding a NUL byte.
    NullCharacter { offset: usize },
    /// An `os.date` `%` before something other than a date specifier or `%`.
    BadDateSpecifier { offset: usize },
}

impl PatternError {
    /// Byte index into the validated body where the error was found.
    pub(crate) const fn offset(&self) -> usize {
        match *self {
            Self::TruncatedSpecifier { offset }
            | Self::UnterminatedSet { offset }
            | Self::EmptySet { offset }
            | Self::InvalidQuantifierTarget { offset }
            | Self::UnmatchedCapture { offset }
            | Self::TruncatedPackSize { offset }
            | Self::LonePercent { offset }
            | Self::BadReplacementEscape { offset }
            | Self::CaptureIndexOutOfRange { offset }
            | Self::NullCharacter { offset }
            | Self::BadDateSpecifier { offset }
            | Self::BadWidth { offset }
            | Self::BadPrecision { offset }
            | Self::UnknownSpecifier { offset, .. }
            | Self::BadFlag { offset, .. }
            | Self::BadEscape { offset, .. }
            | Self::InvalidPackOption { offset, .. } => offset,
        }
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedSpecifier { offset } => {
                write!(f, "truncated specifier at offset {offset}")
            }
            Self::UnknownSpecifier { offset, character } => {
                write!(f, "unknown specifier `%{character}` at offset {offset}")
            }
            Self::BadFlag { offset, character } => {
                write!(f, "repeated flag `{character}` at offset {offset}")
            }
            Self::BadWidth { offset } => write!(f, "width too long at offset {offset}"),
            Self::BadPrecision { offset } => {
                write!(f, "precision too long at offset {offset}")
            }
            Self::UnterminatedSet { offset } => {
                write!(f, "unterminated set at offset {offset}")
            }
            Self::EmptySet { offset } => write!(f, "empty set at offset {offset}"),
            Self::BadEscape { offset, character } => {
                write!(f, "bad escape `%{character}` at offset {offset}")
            }
            Self::InvalidQuantifierTarget { offset } => {
                write!(f, "quantifier has no target at offset {offset}")
            }
            Self::UnmatchedCapture { offset } => {
                write!(f, "unmatched capture at offset {offset}")
            }
            Self::InvalidPackOption { offset, character } => {
                write!(f, "invalid pack option `{character}` at offset {offset}")
            }
            Self::TruncatedPackSize { offset } => {
                write!(f, "truncated pack size at offset {offset}")
            }
            Self::LonePercent { .. } => f.write_str("ends with a lone `%`"),
            Self::BadReplacementEscape { .. } => {
                f.write_str("unexpected character after `%`, expected a digit or `%`")
            }
            Self::CaptureIndexOutOfRange { .. } => {
                f.write_str("capture index is higher than the pattern's capture count")
            }
            Self::NullCharacter { .. } => f.write_str("contains a null character"),
            Self::BadDateSpecifier { .. } => {
                f.write_str("unexpected character after `%`, expected a date specifier or `%`")
            }
        }
    }
}

impl Error for PatternError {}

/// Forward-only reader over a pattern body. Offsets count bytes from the
/// start of the body, matching what [`PatternError`] reports.
struct Cursor<'pattern> {
    remaining_bytes: &'pattern [u8],
    body_length: usize,
}

impl<'pattern> Cursor<'pattern> {
    const fn new(pattern: &'pattern str) -> Self {
        Self { remaining_bytes: pattern.as_bytes(), body_length: pattern.len() }
    }

    const fn offset(&self) -> usize {
        self.body_length.saturating_sub(self.remaining_bytes.len())
    }

    const fn peek(&self) -> Option<u8> {
        self.remaining_bytes.first().copied()
    }

    /// Skips `count` bytes, stopping at the end of the body.
    fn skip_bytes(&mut self, count: usize) {
        self.remaining_bytes = self.remaining_bytes.get(count..).unwrap_or_default();
    }

    /// Skips a run of ASCII digits and returns its length.
    fn skip_digits(&mut self) -> usize {
        let digit_count =
            self.remaining_bytes.iter().take_while(|byte| byte.is_ascii_digit()).count();
        self.skip_bytes(digit_count);
        digit_count
    }

    /// Skips one UTF-8 sequence so error offsets land on char boundaries.
    ///
    /// A one-byte escape skip can leave a continuation byte in lead
    /// position, and that steps a single byte.
    fn skip_char(&mut self) {
        let width = match self.peek() {
            Some(0xC0..=0xDF) => 2,
            Some(0xE0..=0xEF) => 3,
            Some(0xF0..=0xFF) => 4,
            Some(0x00..=0xBF) | None => 1,
        };
        self.skip_bytes(width);
    }
}

/// Validates a `string.format` string and returns how many arguments its
/// specifiers consume.
///
/// # Errors
///
/// The first malformed specifier, as a [`PatternError`].
pub(crate) fn validate_format(pattern: &str) -> Result<usize, PatternError> {
    let mut cursor = Cursor::new(pattern);
    let mut specifier_count = 0usize;

    while let Some(byte) = cursor.peek() {
        let start = cursor.offset();
        cursor.skip_bytes(1);
        if byte != b'%' {
            continue;
        }
        let truncated_error = PatternError::TruncatedSpecifier { offset: start };

        // `%%` is a literal percent and consumes no argument.
        match cursor.peek() {
            Some(b'%') => {
                cursor.skip_bytes(1);
                continue;
            }
            Some(_) => {}
            None => return Err(truncated_error),
        }

        // Flags can come in any order, but each at most once.
        let mut seen_flags = 0u8;
        while let Some(flag) = cursor.peek() {
            let flag_bit = match flag {
                b'-' => 1,
                b'+' => 2,
                b' ' => 4,
                b'#' => 8,
                b'0' => 16,
                _ => break,
            };
            if seen_flags & flag_bit != 0 {
                return Err(PatternError::BadFlag {
                    offset: cursor.offset(),
                    character: char::from(flag),
                });
            }
            seen_flags |= flag_bit;
            cursor.skip_bytes(1);
        }

        // Lua allows at most two digits each for width and precision.
        let width_start = cursor.offset();
        if cursor.skip_digits() > 2 {
            return Err(PatternError::BadWidth { offset: width_start });
        }

        if cursor.peek() == Some(b'.') {
            cursor.skip_bytes(1);
            let precision_start = cursor.offset();
            if cursor.skip_digits() > 2 {
                return Err(PatternError::BadPrecision { offset: precision_start });
            }
        }

        let Some(conversion) = cursor.peek() else {
            return Err(truncated_error);
        };
        if !matches!(
            conversion,
            b'c' | b'd'
                | b'i'
                | b'o'
                | b'u'
                | b'x'
                | b'X'
                | b'e'
                | b'E'
                | b'f'
                | b'g'
                | b'G'
                | b'a'
                | b'A'
                | b'q'
                | b's'
        ) {
            return Err(PatternError::UnknownSpecifier {
                offset: cursor.offset(),
                character: char::from(conversion),
            });
        }
        specifier_count = specifier_count.saturating_add(1);
        cursor.skip_bytes(1);
    }

    Ok(specifier_count)
}

/// Validates a Lua pattern, as taken by `string.match`, `gmatch`, `find`,
/// and `gsub`, and returns how many captures it has.
///
/// # Errors
///
/// The first malformed construct, as a [`PatternError`].
pub(crate) fn validate_lua_pattern(pattern: &str) -> Result<usize, PatternError> {
    let mut cursor = Cursor::new(pattern);
    let mut capture_count = 0usize;
    let mut open_capture_count = 0u32;
    let mut can_quantify = false;

    while let Some(here) = cursor.peek() {
        // A leading `^` and a trailing `$` are anchors, which a quantifier
        // can't repeat.
        let is_anchor = (here == b'^' && cursor.offset() == 0)
            || (here == b'$' && cursor.remaining_bytes.len() == 1);
        if is_anchor {
            cursor.skip_bytes(1);
            can_quantify = false;
            continue;
        }

        can_quantify = match here {
            b'%' => scan_escape(&mut cursor)?,
            b'[' => {
                scan_set(&mut cursor)?;
                true
            }
            b'(' => {
                cursor.skip_bytes(1);
                open_capture_count = open_capture_count.saturating_add(1);
                capture_count = capture_count.saturating_add(1);
                false
            }
            b')' => {
                let Some(still_open) = open_capture_count.checked_sub(1) else {
                    return Err(PatternError::UnmatchedCapture { offset: cursor.offset() });
                };
                open_capture_count = still_open;
                cursor.skip_bytes(1);
                true
            }
            b'*' | b'+' | b'-' | b'?' => {
                if !can_quantify {
                    return Err(PatternError::InvalidQuantifierTarget { offset: cursor.offset() });
                }
                cursor.skip_bytes(1);
                false
            }
            b'.' => {
                cursor.skip_bytes(1);
                true
            }
            _ => {
                cursor.skip_char();
                true
            }
        };
    }

    if open_capture_count != 0 {
        return Err(PatternError::UnmatchedCapture { offset: pattern.len() });
    }

    Ok(capture_count)
}

/// Scans a `%` escape with the cursor on the `%`, and returns whether a
/// quantifier may follow it.
///
/// # Errors
///
/// A [`PatternError`] for a truncated or unrecognized escape.
fn scan_escape(cursor: &mut Cursor<'_>) -> Result<bool, PatternError> {
    let start = cursor.offset();
    cursor.skip_bytes(1);
    let escape_offset = cursor.offset();
    let Some(escape) = cursor.peek() else {
        return Err(PatternError::TruncatedSpecifier { offset: start });
    };
    cursor.skip_bytes(1);

    match escape {
        // Character classes.
        b'a' | b'A' | b'c' | b'C' | b'd' | b'D' | b'g' | b'G' | b'l' | b'L' | b'p' | b'P'
        | b's' | b'S' | b'u' | b'U' | b'w' | b'W' | b'x' | b'X' | b'z' | b'Z' => Ok(true),
        // Back-references. Lua reads no quantifier after one, so a
        // following `*` is a literal `*`.
        b'0'..=b'9' => Ok(false),
        // `%bxy` matches a run balanced between `x` and `y`.
        b'b' => {
            if cursor.remaining_bytes.len() < 2 {
                return Err(PatternError::TruncatedSpecifier { offset: start });
            }
            cursor.skip_bytes(2);
            Ok(false)
        }
        // A `%f` frontier needs a `[set]` after it.
        b'f' => {
            if cursor.peek() != Some(b'[') {
                return Err(PatternError::BadEscape { offset: start, character: 'f' });
            }
            scan_set(cursor)?;
            Ok(false)
        }
        // Any other non-letter, `%` included, is an escaped literal.
        character if !character.is_ascii_alphabetic() => Ok(true),
        other => {
            Err(PatternError::BadEscape { offset: escape_offset, character: char::from(other) })
        }
    }
}

/// Scans a `[set]` with the cursor on the opening `[`, leaving it just past
/// the closing `]`.
///
/// # Errors
///
/// A [`PatternError`] for an empty, unterminated, or truncated set.
fn scan_set(cursor: &mut Cursor<'_>) -> Result<(), PatternError> {
    let start = cursor.offset();
    debug_assert_eq!(cursor.peek(), Some(b'['), "a set opens with `[`");
    cursor.skip_bytes(1);

    if cursor.peek() == Some(b'^') {
        cursor.skip_bytes(1);
    }

    // Lua reads a `]` here as a member, so a bare `[]` or `[^]` is
    // unterminated. Flagging it also rejects sets like `[]]` and `[]a]`,
    // which Lua accepts.
    if cursor.peek() == Some(b']') {
        return Err(PatternError::EmptySet { offset: start });
    }

    while let Some(here) = cursor.peek() {
        match here {
            b']' => {
                cursor.skip_bytes(1);
                return Ok(());
            }
            b'%' => {
                let escape_start = cursor.offset();
                cursor.skip_bytes(1);
                if cursor.peek().is_none() {
                    return Err(PatternError::TruncatedSpecifier { offset: escape_start });
                }
                cursor.skip_bytes(1);
            }
            _ => cursor.skip_char(),
        }
    }

    Err(PatternError::UnterminatedSet { offset: start })
}

/// Validates a `string.pack`, `unpack`, or `packsize` format and returns
/// how many values it packs.
///
/// Control options such as `<`, `>`, `=`, `!`, `x`, and `X` take no value.
///
/// # Errors
///
/// The first malformed option, as a [`PatternError`].
pub(crate) fn validate_pack_format(pattern: &str) -> Result<usize, PatternError> {
    let mut cursor = Cursor::new(pattern);
    let mut value_count = 0usize;

    while let Some(option) = cursor.peek() {
        let option_offset = cursor.offset();
        cursor.skip_bytes(1);

        let has_value = match option {
            // Spaces, endianness controls, and `x` padding take no value.
            b' ' | b'\t' | b'<' | b'>' | b'=' | b'x' => false,
            // The `!` alignment control takes an optional size suffix.
            b'!' => {
                cursor.skip_digits();
                false
            }
            // `X` aligns to the size of the next option and packs no value.
            b'X' => {
                while matches!(cursor.peek(), Some(b' ' | b'\t')) {
                    cursor.skip_bytes(1);
                }
                let Some(aligned_option) = cursor.peek() else {
                    return Err(PatternError::TruncatedPackSize { offset: cursor.offset() });
                };
                // That option must be a sized type. It's consumed here so it
                // isn't counted as a value.
                if !matches!(
                    aligned_option,
                    b'b' | b'B'
                        | b'h'
                        | b'H'
                        | b'i'
                        | b'I'
                        | b'l'
                        | b'L'
                        | b'j'
                        | b'J'
                        | b'T'
                        | b'f'
                        | b'd'
                        | b'n'
                ) {
                    return Err(PatternError::InvalidPackOption {
                        offset: cursor.offset(),
                        character: char::from(aligned_option),
                    });
                }
                cursor.skip_bytes(1);
                cursor.skip_digits();
                false
            }
            // Fixed-size integers, floats, and NUL-terminated `z` strings
            // take no size suffix.
            b'b' | b'B' | b'h' | b'H' | b'l' | b'L' | b'j' | b'J' | b'T' | b'f' | b'd' | b'n'
            | b'z' => true,
            // `i`/`I` integers and length-prefixed `s` strings accept an
            // optional size suffix.
            b'i' | b'I' | b's' => {
                cursor.skip_digits();
                true
            }
            // Fixed-length string `c<n>` requires a size suffix.
            b'c' => {
                let size_start = cursor.offset();
                if cursor.skip_digits() == 0 {
                    return Err(PatternError::TruncatedPackSize { offset: size_start });
                }
                true
            }
            // A digit lands here when it follows an option that takes no
            // size.
            other => {
                return Err(PatternError::InvalidPackOption {
                    offset: option_offset,
                    character: char::from(other),
                });
            }
        };

        if has_value {
            value_count = value_count.saturating_add(1);
        }
    }

    Ok(value_count)
}

/// Counts the unescaped `(` in a Lua pattern, position captures `()`
/// included.
///
/// Sets are skipped, so `[(]` doesn't count. The `(` in `%b()` does count,
/// which is fine because overcounting only suppresses diagnostics.
pub(crate) fn count_pattern_captures(pattern: &str) -> usize {
    let mut bytes = pattern.bytes().peekable();
    from_fn(|| {
        let byte = bytes.next()?;
        match byte {
            b'%' => {
                bytes.next();
            }
            b'[' => {
                bytes.next_if_eq(&b'^');
                // A `]` right after the opener (or `^`) is a literal.
                bytes.next_if_eq(&b']');
                while let Some(set_byte) = bytes.next_if(|&set_byte| set_byte != b']') {
                    if set_byte == b'%' {
                        bytes.next();
                    }
                }
                bytes.next();
            }
            _ => {}
        }
        Some(byte)
    })
    .filter(|&byte| byte == b'(')
    .count()
}

/// Validates a `gsub` replacement string.
///
/// `capture_count` is `None` when the pattern isn't a literal, which
/// disables the index check. `%0` is the whole match, and with zero captures
/// so is `%1`, so only indices above `max(captures, 1)` are flagged.
///
/// # Errors
///
/// The first invalid `%` sequence, as a [`PatternError`].
pub(crate) fn validate_gsub_replacement(
    body: &str,
    capture_count: Option<usize>,
) -> Result<(), PatternError> {
    let mut bytes = body.bytes().enumerate();
    while let Some((index, byte)) = bytes.next() {
        if byte != b'%' {
            continue;
        }
        let Some((next_index, next)) = bytes.next() else {
            return Err(PatternError::LonePercent { offset: index });
        };
        if next == b'%' {
            continue;
        }
        if !next.is_ascii_digit() {
            return Err(PatternError::BadReplacementEscape { offset: next_index });
        }
        if let Some(known_capture_count) = capture_count
            && usize::from(next.saturating_sub(b'0')) > known_capture_count.max(1)
        {
            return Err(PatternError::CaptureIndexOutOfRange { offset: next_index });
        }
    }
    Ok(())
}

/// Validates an `os.date` format string.
///
/// After an optional leading `!` (UTC), it's either exactly `*t` or text
/// whose specifiers come from C89 `strftime` plus `%z`.
///
/// # Errors
///
/// The first invalid character, as a [`PatternError`].
pub(crate) fn validate_os_date(body: &str) -> Result<(), PatternError> {
    if let Some(null_offset) = body.find('\0') {
        return Err(PatternError::NullCharacter { offset: null_offset });
    }
    if body.strip_prefix('!').unwrap_or(body) == "*t" {
        return Ok(());
    }
    let mut bytes = body.bytes().enumerate();
    while let Some((index, byte)) = bytes.next() {
        if byte != b'%' {
            continue;
        }
        let Some((next_index, next)) = bytes.next() else {
            return Err(PatternError::LonePercent { offset: index });
        };
        if next != b'%' && !b"aAbBcdHIjmMpSUwWxXyYzZ".contains(&next) {
            return Err(PatternError::BadDateSpecifier { offset: next_index });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_two_specifiers() {
        assert_eq!(validate_format("%d %s").unwrap(), 2);
    }

    #[test]
    fn format_width_precision() {
        assert_eq!(validate_format("%5.2f").unwrap(), 1);
    }

    #[test]
    fn format_literal_percent() {
        assert_eq!(validate_format("%%").unwrap(), 0);
    }

    #[test]
    fn format_quoted() {
        assert_eq!(validate_format("%q").unwrap(), 1);
    }

    #[test]
    fn format_left_aligned() {
        assert_eq!(validate_format("%-10s").unwrap(), 1);
    }

    #[test]
    fn format_truncated() {
        assert!(matches!(validate_format("%"), Err(PatternError::TruncatedSpecifier { .. })));
    }

    #[test]
    fn format_unknown_specifier() {
        // `%z` is valid in patterns but not in format specs.
        assert!(matches!(
            validate_format("%z"),
            Err(PatternError::UnknownSpecifier { character: 'z', .. })
        ));
    }

    #[test]
    fn pattern_no_captures() {
        assert_eq!(validate_lua_pattern("abc").unwrap(), 0);
    }

    #[test]
    fn pattern_one_capture() {
        assert_eq!(validate_lua_pattern("(%a+)").unwrap(), 1);
    }

    #[test]
    fn pattern_two_captures() {
        assert_eq!(validate_lua_pattern("(%d)(%a)").unwrap(), 2);
    }

    #[test]
    fn pattern_simple_set() {
        assert_eq!(validate_lua_pattern("[a-z]").unwrap(), 0);
    }

    #[test]
    fn pattern_unterminated_set() {
        assert!(matches!(validate_lua_pattern("[a-"), Err(PatternError::UnterminatedSet { .. })));
    }

    #[test]
    fn pattern_empty_set() {
        assert!(matches!(validate_lua_pattern("[]"), Err(PatternError::EmptySet { .. })));
    }

    #[test]
    fn pattern_word_class() {
        assert_eq!(validate_lua_pattern("%w").unwrap(), 0);
    }

    #[test]
    fn pattern_back_reference() {
        assert_eq!(validate_lua_pattern("%9").unwrap(), 0);
    }

    #[test]
    fn pattern_balanced_match() {
        assert_eq!(validate_lua_pattern("%bab").unwrap(), 0);
    }

    #[test]
    fn pattern_frontier() {
        assert_eq!(validate_lua_pattern("%f[%a]").unwrap(), 0);
    }

    #[test]
    fn pattern_frontier_truncated() {
        assert!(matches!(
            validate_lua_pattern("%f"),
            Err(PatternError::TruncatedSpecifier { .. }
                | PatternError::BadEscape { character: 'f', .. })
        ));
    }

    #[test]
    fn pattern_quantifier_at_start() {
        assert!(matches!(
            validate_lua_pattern("*"),
            Err(PatternError::InvalidQuantifierTarget { .. })
        ));
    }

    #[test]
    fn pattern_quantifier_after_back_reference() {
        assert!(matches!(
            validate_lua_pattern("(a)%1*"),
            Err(PatternError::InvalidQuantifierTarget { offset: 5 })
        ));
    }

    #[test]
    fn pattern_back_reference_without_quantifier() {
        assert_eq!(validate_lua_pattern("(a)%1").unwrap(), 1);
    }

    #[test]
    fn pack_two_int_values() {
        assert_eq!(validate_pack_format(">i4 i4").unwrap(), 2);
    }

    #[test]
    fn pack_three_fixed_types() {
        assert_eq!(validate_pack_format("b B h").unwrap(), 3);
    }

    #[test]
    fn pack_sized_string() {
        assert_eq!(validate_pack_format("s2").unwrap(), 1);
    }

    #[test]
    fn pack_padding_no_value() {
        assert_eq!(validate_pack_format("x").unwrap(), 0);
    }

    #[test]
    fn pack_align_to_type() {
        assert_eq!(validate_pack_format("Xi").unwrap(), 0);
    }

    #[test]
    fn pack_align_with_sized_type() {
        assert_eq!(validate_pack_format("Xi4").unwrap(), 0);
    }

    #[test]
    fn pack_alignment_control() {
        assert_eq!(validate_pack_format("!").unwrap(), 0);
    }

    #[test]
    fn pack_truncated_align() {
        assert!(matches!(validate_pack_format("X"), Err(PatternError::TruncatedPackSize { .. })));
    }

    #[test]
    fn pack_size_on_unsized_option() {
        // `x` takes no size, so the `4` is stray.
        assert!(matches!(
            validate_pack_format("x4"),
            Err(PatternError::InvalidPackOption { character: '4', .. })
        ));
    }
}
