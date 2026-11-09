//! Validators for the string DSLs of Lua's stdlib.
//!
//! They cover `string.format` specifiers, Lua patterns
//! (`string.match`/`gmatch`/`find`/`gsub`), `string.pack`/`string.unpack`
//! format strings, `gsub` replacement strings, and `os.date` formats, each as
//! the target's `lstrlib.c` and `loslib.c` read them.
//!
//! Each validator takes a literal's body without its quotes and returns a
//! count (specifiers, captures, or values consumed) where one applies, or
//! the first [`PatternError`] it finds. Callers map the error's offset back
//! into source.

use core::error::Error;
use core::fmt;

use luck_token::LuaVersion;

/// `LUA_MAXCAPTURES` in every dialect.
const MAX_CAPTURES: usize = 32;

/// `MAXINTSIZE`, the largest explicit size of a `string.pack` integer.
const MAX_PACK_INT_SIZE: u32 = 16;

/// The smallest alignment a bare `!` can mean.
const MIN_NATIVE_ALIGNMENT: u32 = 8;

/// The first malformed construct a validator finds. Each `offset` is a
/// byte index into the body the validator was given.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub(crate) enum PatternError {
    /// A `%` construct cut off by the end of the body.
    TruncatedSpecifier { offset: usize },
    /// A `string.format` conversion the target doesn't have.
    UnknownSpecifier { offset: usize, character: char },
    /// More flags in one `string.format` specifier than Lua allows.
    BadFlag { offset: usize, character: char },
    /// A width longer than the two digits Lua allows.
    BadWidth { offset: usize },
    /// A precision longer than the two digits Lua allows.
    BadPrecision { offset: usize },
    /// A flag, width, or precision a Lua 5.4+ conversion rejects.
    BadModifier { offset: usize, modifier: char, conversion: char },
    /// A Lua 5.4+ specifier longer than the bound `getformat` enforces.
    LongSpecifier { offset: usize },
    /// A `[set]` with no closing `]`.
    UnterminatedSet { offset: usize },
    /// A `[]` or `[^]` ending the body, whose `]` is a member, so nothing
    /// closes the set.
    EmptySet { offset: usize },
    /// A `%f` with no `[set]` after it.
    BadEscape { offset: usize, character: char },
    /// A `)` with no open capture, or a `(` that never closes.
    UnmatchedCapture { offset: usize },
    /// A `(` past the limit on captures.
    TooManyCaptures { offset: usize },
    /// A `%0`, or a `%n` naming a capture that hasn't closed.
    BadBackReference { offset: usize },
    /// An unknown `string.pack` option, or an option `X` can't align to.
    InvalidPackOption { offset: usize, character: char },
    /// A `c` with no size, or an `X` with no option after it.
    TruncatedPackSize { offset: usize },
    /// An `i`, `I`, `s`, or `!` size outside 1 to 16.
    PackSizeOutOfLimits { offset: usize },
    /// A size whose alignment isn't a power of two under the current `!`.
    BadPackAlignment { offset: usize },
    /// An `s` or `z` option in a `string.packsize` format.
    VariableLengthPackOption { offset: usize, character: char },
    /// A `gsub` replacement or `os.date` format ending in a single `%`.
    LonePercent { offset: usize },
    /// A `gsub` replacement `%` before something other than a digit or `%`.
    BadReplacementEscape { offset: usize },
    /// A `gsub` replacement `%n` above the pattern's capture count.
    CaptureIndexOutOfRange { offset: usize },
    /// An `os.date` `%` before something other than a date specifier or `%`.
    BadDateSpecifier { offset: usize },
}

impl PatternError {
    /// Byte index into the validated body where the error was found.
    pub(crate) const fn offset(&self) -> usize {
        match *self {
            Self::TruncatedSpecifier { offset }
            | Self::LongSpecifier { offset }
            | Self::UnterminatedSet { offset }
            | Self::EmptySet { offset }
            | Self::UnmatchedCapture { offset }
            | Self::TooManyCaptures { offset }
            | Self::BadBackReference { offset }
            | Self::TruncatedPackSize { offset }
            | Self::PackSizeOutOfLimits { offset }
            | Self::BadPackAlignment { offset }
            | Self::LonePercent { offset }
            | Self::BadReplacementEscape { offset }
            | Self::CaptureIndexOutOfRange { offset }
            | Self::BadDateSpecifier { offset }
            | Self::BadWidth { offset }
            | Self::BadPrecision { offset }
            | Self::UnknownSpecifier { offset, .. }
            | Self::BadFlag { offset, .. }
            | Self::BadModifier { offset, .. }
            | Self::BadEscape { offset, .. }
            | Self::InvalidPackOption { offset, .. }
            | Self::VariableLengthPackOption { offset, .. } => offset,
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
                write!(f, "too many flags at `{character}` at offset {offset}")
            }
            Self::BadWidth { offset } => write!(f, "width too long at offset {offset}"),
            Self::BadPrecision { offset } => {
                write!(f, "precision too long at offset {offset}")
            }
            Self::BadModifier { offset, modifier, conversion } => {
                write!(f, "`%{conversion}` can't take `{modifier}` at offset {offset}")
            }
            Self::LongSpecifier { offset } => write!(f, "specifier too long at offset {offset}"),
            Self::UnterminatedSet { offset } => {
                write!(f, "unterminated set at offset {offset}")
            }
            Self::EmptySet { offset } => write!(f, "empty set at offset {offset}"),
            Self::BadEscape { offset, character } => {
                write!(f, "bad escape `%{character}` at offset {offset}")
            }
            Self::UnmatchedCapture { offset } => {
                write!(f, "unmatched capture at offset {offset}")
            }
            Self::TooManyCaptures { offset } => {
                write!(f, "too many captures at offset {offset}")
            }
            Self::BadBackReference { offset } => {
                write!(f, "back reference names no closed capture at offset {offset}")
            }
            Self::InvalidPackOption { offset, character } => {
                write!(f, "invalid pack option `{character}` at offset {offset}")
            }
            Self::TruncatedPackSize { offset } => {
                write!(f, "truncated pack size at offset {offset}")
            }
            Self::PackSizeOutOfLimits { offset } => {
                write!(f, "integer size out of limits at offset {offset}")
            }
            Self::BadPackAlignment { offset } => {
                write!(f, "alignment is not a power of 2 at offset {offset}")
            }
            Self::VariableLengthPackOption { offset, character } => {
                write!(f, "variable-length option `{character}` at offset {offset}")
            }
            Self::LonePercent { .. } => f.write_str("ends with a lone `%`"),
            Self::BadReplacementEscape { .. } => {
                f.write_str("unexpected character after `%`, expected a digit or `%`")
            }
            Self::CaptureIndexOutOfRange { .. } => {
                f.write_str("capture index is higher than the pattern's capture count")
            }
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
    pattern: &'pattern str,
    offset: usize,
}

impl<'pattern> Cursor<'pattern> {
    const fn new(pattern: &'pattern str) -> Self {
        Self { pattern, offset: 0 }
    }

    const fn offset(&self) -> usize {
        self.offset
    }

    /// How many bytes follow the cursor.
    const fn remaining_length(&self) -> usize {
        self.pattern.len().saturating_sub(self.offset)
    }

    fn peek(&self) -> Option<u8> {
        self.pattern.as_bytes().get(self.offset).copied()
    }

    /// The whole character at the cursor, for an error that shows it.
    fn peek_char(&self) -> char {
        self.pattern
            .get(self.offset..)
            .and_then(|rest| rest.chars().next())
            .unwrap_or(char::REPLACEMENT_CHARACTER)
    }

    /// Skips `count` bytes, stopping at the end of the body.
    fn skip_bytes(&mut self, count: usize) {
        self.offset = self.offset.saturating_add(count).min(self.pattern.len());
    }

    /// Skips the run of bytes that `is_included` accepts and returns it.
    fn take_while(&mut self, mut is_included: impl FnMut(u8) -> bool) -> &'pattern str {
        let start = self.offset;
        while self.peek().is_some_and(&mut is_included) {
            self.skip_bytes(1);
        }
        self.pattern.get(start..self.offset).unwrap_or_default()
    }

    /// Skips a run of ASCII digits and returns its length.
    fn skip_digits(&mut self) -> usize {
        self.take_while(|byte| byte.is_ascii_digit()).len()
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
pub(crate) fn validate_format(pattern: &str, version: LuaVersion) -> Result<usize, PatternError> {
    let mut cursor = Cursor::new(pattern);
    let mut specifier_count = 0usize;

    while let Some(byte) = cursor.peek() {
        let percent_offset = cursor.offset();
        cursor.skip_bytes(1);
        if byte != b'%' {
            continue;
        }
        match cursor.peek() {
            // `%%` is a literal percent and consumes no argument.
            Some(b'%') => {
                cursor.skip_bytes(1);
                continue;
            }
            // Luau: takes any value and no modifiers.
            Some(b'*') if version.has_star_format() => cursor.skip_bytes(1),
            Some(_) => {
                let modifiers = scan_format_modifiers(&mut cursor, version)?;
                scan_format_conversion(&mut cursor, percent_offset, modifiers, version)?;
            }
            None => return Err(PatternError::TruncatedSpecifier { offset: percent_offset }),
        }
        specifier_count = specifier_count.saturating_add(1);
    }

    Ok(specifier_count)
}

/// Reads the text between a `%` and its conversion character the way the
/// target's `getformat` or `scanformat` does, and returns it.
///
/// # Errors
///
/// A [`PatternError`] for too many flags or digits.
fn scan_format_modifiers<'pattern>(
    cursor: &mut Cursor<'pattern>,
    version: LuaVersion,
) -> Result<&'pattern str, PatternError> {
    let start = cursor.offset();
    if version.has_checked_format_modifiers() {
        let modifiers = cursor.take_while(|byte| {
            matches!(byte, b'-' | b'+' | b'#' | b'0' | b' ' | b'1'..=b'9' | b'.')
        });
        // `MAX_FORMAT - 10`, less the conversion character.
        if modifiers.len() >= 21 {
            return Err(PatternError::LongSpecifier { offset: start });
        }
        return Ok(modifiers);
    }

    let flags = cursor.take_while(|byte| matches!(byte, b'-' | b'+' | b' ' | b'#' | b'0'));
    // `scanformat` rejects `sizeof(FLAGS)`, six, or more flag bytes.
    if let Some(character) = flags.chars().nth(5) {
        return Err(PatternError::BadFlag { offset: start.saturating_add(5), character });
    }

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
    Ok(cursor.pattern.get(start..cursor.offset()).unwrap_or_default())
}

/// Checks the conversion character at the cursor against the target, and
/// on Lua 5.4+ the `modifiers` before it.
///
/// # Errors
///
/// A [`PatternError`] for a conversion or modifier the target rejects.
fn scan_format_conversion(
    cursor: &mut Cursor<'_>,
    percent_offset: usize,
    modifiers: &str,
    version: LuaVersion,
) -> Result<(), PatternError> {
    let Some(conversion) = cursor.peek() else {
        return Err(PatternError::TruncatedSpecifier { offset: percent_offset });
    };
    let is_known_conversion = match conversion {
        b'c' | b'd' | b'i' | b'o' | b'u' | b'x' | b'X' | b'e' | b'E' | b'f' | b'g' | b'G'
        | b'q' | b's' => true,
        b'a' | b'A' => version.has_hex_float_format(),
        b'p' => version.has_pointer_format(),
        _ => false,
    };
    if !is_known_conversion {
        return Err(PatternError::UnknownSpecifier {
            offset: cursor.offset(),
            character: cursor.peek_char(),
        });
    }
    if version.has_checked_format_modifiers()
        && let Some(rejected_index) = rejected_modifier(modifiers.as_bytes(), conversion)
    {
        return Err(PatternError::BadModifier {
            offset: cursor.offset().saturating_sub(modifiers.len()).saturating_add(rejected_index),
            modifier: modifiers.chars().nth(rejected_index).unwrap_or('?'),
            conversion: char::from(conversion),
        });
    }
    cursor.skip_bytes(1);
    Ok(())
}

/// The index in `modifiers` of the first byte Lua 5.4's `checkformat` rejects
/// for a known `conversion`. The flag and precision sets are the ones
/// `str_format` hands it.
fn rejected_modifier(modifiers: &[u8], conversion: u8) -> Option<usize> {
    let (flags, has_precision): (&[u8], bool) = match conversion {
        // "specifier '%q' cannot have modifiers"
        b'q' => return (!modifiers.is_empty()).then_some(0),
        b'c' | b'p' => (b"-", false),
        b'd' | b'i' => (b"-+0 ", true),
        b'u' => (b"-0", true),
        b'o' | b'x' | b'X' => (b"-#0", true),
        b'a' | b'A' | b'e' | b'E' | b'f' | b'g' | b'G' => (b"-+#0 ", true),
        // The remaining known conversion, `s`.
        _ => (b"-", true),
    };
    let after_two_digits = |from: usize| {
        let digit_count = modifiers
            .get(from..)
            .unwrap_or_default()
            .iter()
            .take(2)
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        from.saturating_add(digit_count)
    };

    let mut index = modifiers.iter().take_while(|byte| flags.contains(byte)).count();
    // A width can't start with `0`.
    if modifiers.get(index) != Some(&b'0') {
        index = after_two_digits(index);
        if has_precision && modifiers.get(index) == Some(&b'.') {
            index = after_two_digits(index.saturating_add(1));
        }
    }
    (index < modifiers.len()).then_some(index)
}

/// Validates a Lua pattern, as taken by `string.match`, `gmatch`, `find`,
/// and `gsub`, and returns how many captures it has.
///
/// # Errors
///
/// The first malformed construct, as a [`PatternError`].
pub(crate) fn validate_lua_pattern(pattern: &str) -> Result<usize, PatternError> {
    let mut cursor = Cursor::new(pattern);
    // One entry per capture so far, true once its `)` has been read.
    let mut captures: Vec<bool> = Vec::new();

    while let Some(here) = cursor.peek() {
        match here {
            b'%' => scan_escape(&mut cursor, &captures)?,
            b'[' => scan_set(&mut cursor)?,
            b'(' => {
                if captures.len() >= MAX_CAPTURES {
                    return Err(PatternError::TooManyCaptures { offset: cursor.offset() });
                }
                captures.push(false);
                cursor.skip_bytes(1);
            }
            b')' => {
                let Some(closed) = captures.iter_mut().rev().find(|is_closed| !**is_closed) else {
                    return Err(PatternError::UnmatchedCapture { offset: cursor.offset() });
                };
                *closed = true;
                cursor.skip_bytes(1);
            }
            // `match` reads anything else as a single-character class, so a
            // `*` with nothing to repeat is a literal `*`.
            _ => cursor.skip_char(),
        }
    }

    if !captures.iter().all(|&is_closed| is_closed) {
        return Err(PatternError::UnmatchedCapture { offset: pattern.len() });
    }

    Ok(captures.len())
}

/// Scans a `%` escape with the cursor on the `%`.
///
/// # Errors
///
/// A [`PatternError`] for a truncated or malformed escape.
fn scan_escape(cursor: &mut Cursor<'_>, captures: &[bool]) -> Result<(), PatternError> {
    let start = cursor.offset();
    cursor.skip_bytes(1);
    let escape_offset = cursor.offset();
    let Some(escape) = cursor.peek() else {
        return Err(PatternError::TruncatedSpecifier { offset: start });
    };
    cursor.skip_bytes(1);

    match escape {
        // Back-references. `check_capture` rejects `%0`, an unseen capture,
        // and one still open.
        b'0'..=b'9' => {
            let capture = usize::from(escape.saturating_sub(b'0'))
                .checked_sub(1)
                .and_then(|index| captures.get(index));
            if capture.is_some_and(|&is_closed| is_closed) {
                Ok(())
            } else {
                Err(PatternError::BadBackReference { offset: escape_offset })
            }
        }
        // `%bxy` matches a run balanced between `x` and `y`.
        b'b' => {
            if cursor.remaining_length() < 2 {
                return Err(PatternError::TruncatedSpecifier { offset: start });
            }
            cursor.skip_bytes(2);
            Ok(())
        }
        // A `%f` frontier needs a `[set]` after it.
        b'f' => {
            if cursor.peek() != Some(b'[') {
                return Err(PatternError::BadEscape { offset: start, character: 'f' });
            }
            scan_set(cursor)?;
            Ok(())
        }
        // A class or a literal. Lua's `match_class` treats an unknown letter
        // as itself rather than rejecting it.
        _ => Ok(()),
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

    if cursor.peek() == Some(b']') && cursor.remaining_length() == 1 {
        return Err(PatternError::EmptySet { offset: start });
    }

    // `classend` reads the first member before it looks for a `]`, so one
    // here is a member, as in `[]]` and `[]a]`.
    let mut is_first_member = true;
    while let Some(here) = cursor.peek() {
        match here {
            b']' if !is_first_member => {
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
        is_first_member = false;
    }

    Err(PatternError::UnterminatedSet { offset: start })
}

/// What one `string.pack` option does.
#[derive(Debug, Clone, Copy)]
enum PackOption {
    /// An integer or float of `size` bytes, or an `s` string with a
    /// `size`-byte length prefix.
    Sized { size: u32, is_variable_length: bool },
    /// `z`, a NUL-terminated string.
    ZeroTerminated,
    /// `c<n>`, which is never aligned.
    FixedString,
    /// `x`, one byte of padding.
    Padding,
    /// `X`, padding to the alignment of the option after it.
    AlignToNext,
    /// `!`, setting the maximum alignment.
    SetMaxAlignment(u32),
    /// A space or an endianness control.
    Control,
}

/// Validates a `string.pack`, `unpack`, or `packsize` format and returns
/// how many values it packs.
///
/// `is_packsize` also rejects the variable-length options, whose size
/// `string.packsize` can't know.
///
/// # Errors
///
/// The first malformed option, as a [`PatternError`].
pub(crate) fn validate_pack_format(
    pattern: &str,
    is_packsize: bool,
) -> Result<usize, PatternError> {
    let mut cursor = Cursor::new(pattern);
    let mut value_count = 0usize;
    // `initheader` starts every format at 1.
    let mut max_alignment = 1u32;

    while cursor.peek().is_some() {
        let option_offset = cursor.offset();
        let character = cursor.peek_char();
        let (has_value, is_variable_length, alignment) = match scan_pack_option(&mut cursor)? {
            PackOption::Sized { size, is_variable_length } => {
                (true, is_variable_length, Some(size))
            }
            PackOption::ZeroTerminated => (true, true, None),
            PackOption::FixedString => (true, false, None),
            PackOption::AlignToNext => (false, false, Some(scan_x_alignment(&mut cursor)?)),
            PackOption::SetMaxAlignment(alignment) => {
                max_alignment = alignment;
                (false, false, None)
            }
            PackOption::Padding | PackOption::Control => (false, false, None),
        };
        if is_packsize && is_variable_length {
            return Err(PatternError::VariableLengthPackOption {
                offset: option_offset,
                character,
            });
        }
        if alignment.is_some_and(|size| size > 1 && !size.min(max_alignment).is_power_of_two()) {
            return Err(PatternError::BadPackAlignment { offset: option_offset });
        }
        if has_value {
            value_count = value_count.saturating_add(1);
        }
    }

    Ok(value_count)
}

/// The alignment of the option that an `X` just read aligns to, as
/// `getdetails` takes it from `getoption`.
///
/// # Errors
///
/// A [`PatternError`] when nothing follows, or the option takes no alignment.
fn scan_x_alignment(cursor: &mut Cursor<'_>) -> Result<u32, PatternError> {
    let offset = cursor.offset();
    let character = cursor.peek_char();
    match scan_pack_option(cursor)? {
        PackOption::Sized { size, .. } => Ok(size),
        PackOption::Padding => Ok(1),
        PackOption::ZeroTerminated
        | PackOption::FixedString
        | PackOption::AlignToNext
        | PackOption::SetMaxAlignment(_)
        | PackOption::Control => Err(PatternError::InvalidPackOption { offset, character }),
    }
}

/// Reads one option with the cursor on its first byte, as `getoption` does.
///
/// A size the build decides reads as its smallest, so a size-dependent error
/// never fires on only one platform.
///
/// # Errors
///
/// A [`PatternError`] for an unknown option or an out-of-limits size.
fn scan_pack_option(cursor: &mut Cursor<'_>) -> Result<PackOption, PatternError> {
    let option_offset = cursor.offset();
    let character = cursor.peek_char();
    let Some(option) = cursor.peek() else {
        return Err(PatternError::TruncatedPackSize { offset: option_offset });
    };
    cursor.skip_bytes(1);

    let parsed = match option {
        b'b' | b'B' => fixed_size(1),
        b'h' | b'H' => fixed_size(2),
        b'l' | b'L' | b'j' | b'J' | b'T' | b'f' => fixed_size(4),
        b'd' | b'n' => fixed_size(8),
        b'i' | b'I' => fixed_size(scan_limited_size(cursor, 4)?),
        b's' => PackOption::Sized { size: scan_limited_size(cursor, 4)?, is_variable_length: true },
        b'z' => PackOption::ZeroTerminated,
        b'c' => {
            let size_start = cursor.offset();
            if cursor.skip_digits() == 0 {
                return Err(PatternError::TruncatedPackSize { offset: size_start });
            }
            PackOption::FixedString
        }
        b'x' => PackOption::Padding,
        b'X' => PackOption::AlignToNext,
        b'!' => PackOption::SetMaxAlignment(scan_limited_size(cursor, MIN_NATIVE_ALIGNMENT)?),
        b' ' | b'<' | b'>' | b'=' => PackOption::Control,
        // A digit lands here when it follows an option that takes no size.
        _ => return Err(PatternError::InvalidPackOption { offset: option_offset, character }),
    };
    Ok(parsed)
}

const fn fixed_size(size: u32) -> PackOption {
    PackOption::Sized { size, is_variable_length: false }
}

/// `getnumlimit`: reads an optional size from 1 to 16, or returns `default`.
///
/// # Errors
///
/// A [`PatternError`] for a size outside those limits.
fn scan_limited_size(cursor: &mut Cursor<'_>, default: u32) -> Result<u32, PatternError> {
    let start = cursor.offset();
    let digits = cursor.take_while(|byte| byte.is_ascii_digit());
    if digits.is_empty() {
        return Ok(default);
    }
    digits
        .parse()
        .ok()
        .filter(|size| (1..=MAX_PACK_INT_SIZE).contains(size))
        .ok_or(PatternError::PackSizeOutOfLimits { offset: start })
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
    version: LuaVersion,
) -> Result<(), PatternError> {
    let is_strict = version.has_strict_gsub_replacement();
    let mut bytes = body.bytes().enumerate();
    while let Some((index, byte)) = bytes.next() {
        if byte != b'%' {
            continue;
        }
        let Some((next_index, next)) = bytes.next() else {
            return if is_strict {
                Err(PatternError::LonePercent { offset: index })
            } else {
                Ok(())
            };
        };
        if next == b'%' {
            continue;
        }
        if !next.is_ascii_digit() {
            if is_strict {
                return Err(PatternError::BadReplacementEscape { offset: next_index });
            }
            continue;
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
/// with conversion specifiers. Builds differ in the set they accept, so
/// every specifier that exists on any platform of the dialect passes.
///
/// # Errors
///
/// The first invalid character, as a [`PatternError`].
pub(crate) fn validate_os_date(body: &str, version: LuaVersion) -> Result<(), PatternError> {
    if !version.has_checked_date_format() || body.strip_prefix('!').unwrap_or(body) == "*t" {
        return Ok(());
    }
    let has_options = version.has_date_format_options();
    let mut bytes = body.bytes().enumerate();
    while let Some((index, byte)) = bytes.next() {
        if byte != b'%' {
            continue;
        }
        let Some((next_index, next)) = bytes.next() else {
            return if has_options {
                Err(PatternError::LonePercent { offset: index })
            } else {
                Ok(())
            };
        };
        let mut is_second_byte_in =
            |set: &[u8]| bytes.next().is_some_and(|(_, second)| set.contains(&second));
        let is_valid = match next {
            b'E' if has_options => is_second_byte_in(b"cCxXyY"),
            b'O' if has_options => is_second_byte_in(b"deHImMSuUVwWy"),
            b'#' if has_options => is_second_byte_in(b"cxdHIjmMpSUwWyY"),
            single => {
                b"aAbBcdHIjmMpSUwWxXyYzZ%".contains(&single)
                    || (has_options && b"CDeFgGhnrRtTuV".contains(&single))
            }
        };
        if !is_valid {
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
        assert_eq!(validate_format("%d %s", LuaVersion::Lua54).unwrap(), 2);
    }

    #[test]
    fn format_width_precision() {
        assert_eq!(validate_format("%5.2f", LuaVersion::Lua54).unwrap(), 1);
    }

    #[test]
    fn format_literal_percent() {
        assert_eq!(validate_format("%%", LuaVersion::Lua54).unwrap(), 0);
    }

    #[test]
    fn format_quoted() {
        assert_eq!(validate_format("%q", LuaVersion::Lua54).unwrap(), 1);
    }

    #[test]
    fn format_left_aligned() {
        assert_eq!(validate_format("%-10s", LuaVersion::Lua54).unwrap(), 1);
    }

    #[test]
    fn format_truncated() {
        assert!(matches!(
            validate_format("%", LuaVersion::Lua54),
            Err(PatternError::TruncatedSpecifier { .. })
        ));
    }

    #[test]
    fn format_unknown_specifier() {
        // `%z` is valid in patterns but not in format specs.
        assert!(matches!(
            validate_format("%z", LuaVersion::Lua54),
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
        assert!(matches!(
            validate_lua_pattern("%9"),
            Err(PatternError::BadBackReference { offset: 1 })
        ));
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
    fn pattern_reads_a_quantifier_with_nothing_to_repeat_as_a_literal() {
        for pattern in ["*", "+a", "-", "?", "^-?%d+$", "(*)", "(a)%1*", "%f[a]*", "a**"] {
            validate_lua_pattern(pattern).unwrap_or_else(|error| panic!("{pattern}: {error}"));
        }
    }

    #[test]
    fn pattern_back_reference_without_quantifier() {
        assert_eq!(validate_lua_pattern("(a)%1").unwrap(), 1);
    }

    #[test]
    fn pack_two_int_values() {
        assert_eq!(validate_pack_format(">i4 i4", false).unwrap(), 2);
    }

    #[test]
    fn pack_three_fixed_types() {
        assert_eq!(validate_pack_format("b B h", false).unwrap(), 3);
    }

    #[test]
    fn pack_sized_string() {
        assert_eq!(validate_pack_format("s2", false).unwrap(), 1);
    }

    #[test]
    fn pack_padding_no_value() {
        assert_eq!(validate_pack_format("x", false).unwrap(), 0);
    }

    #[test]
    fn pack_align_to_type() {
        assert_eq!(validate_pack_format("Xi", false).unwrap(), 0);
    }

    #[test]
    fn pack_align_with_sized_type() {
        assert_eq!(validate_pack_format("Xi4", false).unwrap(), 0);
    }

    #[test]
    fn pack_alignment_control() {
        assert_eq!(validate_pack_format("!", false).unwrap(), 0);
    }

    #[test]
    fn pack_truncated_align() {
        assert!(matches!(
            validate_pack_format("X", false),
            Err(PatternError::TruncatedPackSize { .. })
        ));
    }

    #[test]
    fn pack_size_on_unsized_option() {
        // `x` takes no size, so the `4` is stray.
        assert!(matches!(
            validate_pack_format("x4", false),
            Err(PatternError::InvalidPackOption { character: '4', .. })
        ));
    }

    #[test]
    fn format_conversions_follow_the_target() {
        validate_format("%a", LuaVersion::Lua51).unwrap_err();
        assert_eq!(validate_format("%a", LuaVersion::Lua52).unwrap(), 1);
        validate_format("%a", LuaVersion::Luau).unwrap_err();
        validate_format("%p", LuaVersion::Lua53).unwrap_err();
        assert_eq!(validate_format("%p", LuaVersion::Lua54).unwrap(), 1);
        assert_eq!(validate_format("%*", LuaVersion::Luau).unwrap(), 1);
        validate_format("%5*", LuaVersion::Luau).unwrap_err();
        validate_format("%*", LuaVersion::Lua54).unwrap_err();
    }

    #[test]
    fn format_checks_modifiers_only_on_lua54() {
        assert_eq!(validate_format("%#d", LuaVersion::Lua53).unwrap(), 1);
        assert_eq!(validate_format("%5q", LuaVersion::Luau).unwrap(), 1);
        for pattern in ["%#d", "%5q", "%.3c", "%05s", "%+x", "%123d", "%.123f"] {
            assert!(validate_format(pattern, LuaVersion::Lua54).is_err(), "{pattern}");
        }
        for pattern in ["%-5.2s", "%05.2f", "%#x", "%+d", "% i", "%5c", "%-p"] {
            assert_eq!(validate_format(pattern, LuaVersion::Lua54).unwrap(), 1, "{pattern}");
        }
    }

    #[test]
    fn format_limits_flag_runs_by_target() {
        assert_eq!(validate_format("%-----d", LuaVersion::Lua53).unwrap(), 1);
        assert!(matches!(
            validate_format("%------d", LuaVersion::Lua53),
            Err(PatternError::BadFlag { offset: 6, .. })
        ));
        assert_eq!(validate_format("%------d", LuaVersion::Lua54).unwrap(), 1);
        assert!(matches!(
            validate_format(&format!("%{}d", "-".repeat(21)), LuaVersion::Lua54),
            Err(PatternError::LongSpecifier { .. })
        ));
    }

    #[test]
    fn pattern_rejects_bad_back_references() {
        for pattern in ["%0", "%1", "(a)%2", "(%1)"] {
            assert!(
                matches!(validate_lua_pattern(pattern), Err(PatternError::BadBackReference { .. })),
                "{pattern}"
            );
        }
        assert_eq!(validate_lua_pattern("(a)(b)%2").unwrap(), 2);
    }

    #[test]
    fn pattern_limits_captures() {
        assert_eq!(validate_lua_pattern(&"()".repeat(32)).unwrap(), 32);
        assert!(matches!(
            validate_lua_pattern(&"()".repeat(33)),
            Err(PatternError::TooManyCaptures { .. })
        ));
    }

    #[test]
    fn pattern_reads_unknown_letter_as_itself() {
        assert_eq!(validate_lua_pattern("%y%Q").unwrap(), 0);
    }

    #[test]
    fn pattern_keeps_leading_close_bracket_as_member() {
        assert_eq!(validate_lua_pattern("[]]").unwrap(), 0);
        assert_eq!(validate_lua_pattern("[^]a]").unwrap(), 0);
        assert!(matches!(validate_lua_pattern("[]a"), Err(PatternError::UnterminatedSet { .. })));
    }

    #[test]
    fn pack_limits_integer_sizes() {
        for pattern in ["i17", "i0", "I99", "s17", "!17", "!0"] {
            assert!(
                matches!(
                    validate_pack_format(pattern, false),
                    Err(PatternError::PackSizeOutOfLimits { .. })
                ),
                "{pattern}"
            );
        }
        assert_eq!(validate_pack_format("i16 I1 s8 !16", false).unwrap(), 3);
    }

    #[test]
    fn pack_align_requires_a_sized_option() {
        for pattern in ["X i", "Xz", "Xc1", "XX"] {
            assert!(
                matches!(
                    validate_pack_format(pattern, false),
                    Err(PatternError::InvalidPackOption { .. })
                ),
                "{pattern}"
            );
        }
        assert_eq!(validate_pack_format("Xs Xx", false).unwrap(), 0);
    }

    #[test]
    fn pack_rejects_tab_separator() {
        assert!(matches!(
            validate_pack_format("i\ti", false),
            Err(PatternError::InvalidPackOption { character: '\t', .. })
        ));
    }

    #[test]
    fn pack_checks_alignment_only_under_bang() {
        assert_eq!(validate_pack_format("i3", false).unwrap(), 1);
        assert_eq!(validate_pack_format("!2 i3", false).unwrap(), 1);
        for pattern in ["!4 i3", "! i3", "!4 X i3"] {
            assert!(
                matches!(
                    validate_pack_format(pattern, false),
                    Err(PatternError::BadPackAlignment { .. }
                        | PatternError::InvalidPackOption { .. })
                ),
                "{pattern}"
            );
        }
    }

    #[test]
    fn packsize_rejects_variable_length_options() {
        for pattern in ["s", "z", "<i4 s2"] {
            assert!(
                matches!(
                    validate_pack_format(pattern, true),
                    Err(PatternError::VariableLengthPackOption { .. })
                ),
                "{pattern}"
            );
        }
        assert_eq!(validate_pack_format("<i4 c8", true).unwrap(), 2);
    }

    #[test]
    fn gsub_replacement_escapes_follow_the_target() {
        validate_gsub_replacement("%x", None, LuaVersion::Lua51).unwrap();
        validate_gsub_replacement("a%", None, LuaVersion::Lua51).unwrap();
        assert!(matches!(
            validate_gsub_replacement("%x", None, LuaVersion::Luau),
            Err(PatternError::BadReplacementEscape { .. })
        ));
        assert!(matches!(
            validate_gsub_replacement("a%", None, LuaVersion::Lua54),
            Err(PatternError::LonePercent { .. })
        ));
    }

    #[test]
    fn os_date_specifiers_follow_the_target() {
        validate_os_date("%Q%", LuaVersion::Lua51).unwrap();
        validate_os_date("%F %T %Ec %Od %#x", LuaVersion::Lua54).unwrap();
        assert!(matches!(
            validate_os_date("%E!", LuaVersion::Lua54),
            Err(PatternError::BadDateSpecifier { .. })
        ));
        assert!(matches!(
            validate_os_date("%F", LuaVersion::Luau),
            Err(PatternError::BadDateSpecifier { .. })
        ));
        validate_os_date("100%", LuaVersion::Luau).unwrap();
    }
}
