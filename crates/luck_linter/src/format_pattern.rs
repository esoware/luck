//! Validators for Lua's three string DSLs: `string.format` specifiers,
//! Lua patterns (`string.match`/`gmatch`/`find`/`gsub`), and
//! `string.pack`/`string.unpack` format strings.
//!
//! Each validator takes the literal pattern body (already stripped of
//! surrounding quotes by the caller) and returns either a count
//! (specifiers, captures, or values consumed) or a precise `PatternError`
//! whose `offset` is a byte index INTO the pattern body. Callers map the
//! offset back into source coordinates.

use std::fmt;

/// Errors produced by the three validators. `offset` is a byte index
/// into the pattern body passed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    /// A `%` at the end of input with nothing after it.
    TruncatedSpecifier { offset: usize },
    /// `%X` where X is not a known specifier letter in this DSL.
    UnknownSpecifier { offset: usize, ch: char },
    /// Duplicate or otherwise malformed flag in a format spec.
    BadFlag { offset: usize, ch: char },
    /// Width digits overflow or are otherwise unparseable.
    BadWidth { offset: usize },
    /// Precision digits overflow or are otherwise unparseable.
    BadPrecision { offset: usize },
    /// `[set]` opened but never closed.
    UnterminatedSet { offset: usize },
    /// `[]` with no characters - Lua treats this as an error in our
    /// validator. (Lua only allows a leading `]` to mean "literal ]"
    /// when there is at least one other character in the set.)
    EmptySet { offset: usize },
    /// `%X` where X is not valid in a pattern context.
    BadEscape { offset: usize, ch: char },
    /// `*`/`+`/`-`/`?` at a position where there is no preceding atom
    /// to quantify (e.g. at the start of a pattern, or right after `(`).
    InvalidQuantifierTarget { offset: usize },
    /// Capture group nesting error or unmatched closing paren.
    UnmatchedCapture { offset: usize },
    /// `string.pack` option letter is not recognized.
    InvalidPackOption { offset: usize, ch: char },
    /// A pack option that requires a size suffix (`s`, `X`) reached EOI
    /// before its argument was supplied.
    TruncatedPackSize { offset: usize },
}

impl fmt::Display for PatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedSpecifier { offset } => {
                write!(formatter, "truncated specifier at offset {offset}")
            }
            Self::UnknownSpecifier { offset, ch } => {
                write!(formatter, "unknown specifier '%{ch}' at offset {offset}")
            }
            Self::BadFlag { offset, ch } => {
                write!(formatter, "bad flag '{ch}' at offset {offset}")
            }
            Self::BadWidth { offset } => write!(formatter, "bad width at offset {offset}"),
            Self::BadPrecision { offset } => {
                write!(formatter, "bad precision at offset {offset}")
            }
            Self::UnterminatedSet { offset } => {
                write!(formatter, "unterminated set at offset {offset}")
            }
            Self::EmptySet { offset } => write!(formatter, "empty set at offset {offset}"),
            Self::BadEscape { offset, ch } => {
                write!(formatter, "bad escape '%{ch}' at offset {offset}")
            }
            Self::InvalidQuantifierTarget { offset } => {
                write!(formatter, "quantifier has no target at offset {offset}")
            }
            Self::UnmatchedCapture { offset } => {
                write!(formatter, "unmatched capture at offset {offset}")
            }
            Self::InvalidPackOption { offset, ch } => {
                write!(formatter, "invalid pack option '{ch}' at offset {offset}")
            }
            Self::TruncatedPackSize { offset } => {
                write!(formatter, "truncated pack size at offset {offset}")
            }
        }
    }
}

impl std::error::Error for PatternError {}

/// Validate a `string.format` pattern. Returns the number of `%X`
/// specifiers that consume an argument (i.e. excludes `%%`).
pub fn validate_format(pattern: &str) -> Result<usize, PatternError> {
    let bytes = pattern.as_bytes();
    let mut idx = 0;
    let mut specifiers = 0;

    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            idx += 1;
            continue;
        }

        let start = idx;
        idx += 1;
        if idx >= bytes.len() {
            return Err(PatternError::TruncatedSpecifier { offset: start });
        }

        // `%%` is a literal percent; consumes no argument.
        if bytes[idx] == b'%' {
            idx += 1;
            continue;
        }

        // Flags: `-`, `+`, ` `, `#`, `0`. Each flag may appear at most
        // once but order within the group is free.
        let mut flag_seen = [false; 5];
        loop {
            let slot = match bytes[idx] {
                b'-' => 0,
                b'+' => 1,
                b' ' => 2,
                b'#' => 3,
                b'0' => 4,
                _ => break,
            };
            if flag_seen[slot] {
                return Err(PatternError::BadFlag {
                    offset: idx,
                    ch: bytes[idx] as char,
                });
            }
            flag_seen[slot] = true;
            idx += 1;
            if idx >= bytes.len() {
                return Err(PatternError::TruncatedSpecifier { offset: start });
            }
        }

        // Width: zero or more decimal digits. Lua caps width at 99 in
        // the reference implementation; reject any 3-digit width.
        let width_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx - width_start > 2 {
            return Err(PatternError::BadWidth {
                offset: width_start,
            });
        }
        if idx >= bytes.len() {
            return Err(PatternError::TruncatedSpecifier { offset: start });
        }

        // Precision: optional `.` followed by zero or more digits. Same
        // 2-digit cap as width.
        if bytes[idx] == b'.' {
            idx += 1;
            let prec_start = idx;
            while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                idx += 1;
            }
            if idx - prec_start > 2 {
                return Err(PatternError::BadPrecision { offset: prec_start });
            }
            if idx >= bytes.len() {
                return Err(PatternError::TruncatedSpecifier { offset: start });
            }
        }

        let conv = bytes[idx];
        match conv {
            b'c' | b'd' | b'i' | b'o' | b'u' | b'x' | b'X' | b'e' | b'E' | b'f' | b'g' | b'G'
            | b'a' | b'A' | b'q' | b's' => {
                specifiers += 1;
                idx += 1;
            }
            _ => {
                return Err(PatternError::UnknownSpecifier {
                    offset: idx,
                    ch: conv as char,
                });
            }
        }
    }

    Ok(specifiers)
}

/// Validate a Lua pattern (the kind accepted by `string.match`,
/// `string.gmatch`, `string.find`, and the search side of
/// `string.gsub`). Returns the number of `(...)` captures.
pub fn validate_lua_pattern(pattern: &str) -> Result<usize, PatternError> {
    let bytes = pattern.as_bytes();
    let mut idx = 0;
    let mut captures = 0;
    let mut open_captures = 0u32;
    // For quantifier validation: was the previous token a quantifiable
    // atom (literal, class, set, or `%n` back-reference)?
    let mut prev_quantifiable = false;

    while idx < bytes.len() {
        let here = bytes[idx];

        // `^` and `$` anchors at start/end are not atoms. Track them so
        // quantifiers don't bind to them.
        if here == b'^' && idx == 0 {
            idx += 1;
            prev_quantifiable = false;
            continue;
        }
        if here == b'$' && idx == bytes.len() - 1 {
            idx += 1;
            prev_quantifiable = false;
            continue;
        }

        match here {
            b'%' => {
                let start = idx;
                idx += 1;
                if idx >= bytes.len() {
                    return Err(PatternError::TruncatedSpecifier { offset: start });
                }
                let escape = bytes[idx];
                match escape {
                    // Character classes: %a %A %c %C %d %D %g %G %l %L
                    // %p %P %s %S %u %U %w %W %x %X %z %Z. All valid
                    // pattern escapes; quantifiable.
                    b'a' | b'A' | b'c' | b'C' | b'd' | b'D' | b'g' | b'G' | b'l' | b'L' | b'p'
                    | b'P' | b's' | b'S' | b'u' | b'U' | b'w' | b'W' | b'x' | b'X' | b'z'
                    | b'Z' => {
                        idx += 1;
                        prev_quantifiable = true;
                    }
                    // `%n` back-references (`%0` through `%9`). They refer
                    // to earlier captures; not quantifiable.
                    b'0'..=b'9' => {
                        idx += 1;
                        prev_quantifiable = true;
                    }
                    // `%bxy`: balanced match between literal x and y.
                    // Consumes three more bytes total.
                    b'b' => {
                        idx += 1;
                        if idx + 1 >= bytes.len() {
                            return Err(PatternError::TruncatedSpecifier { offset: start });
                        }
                        idx += 2;
                        prev_quantifiable = false;
                    }
                    // `%f[set]`: frontier pattern. Must be followed by a
                    // character set.
                    b'f' => {
                        idx += 1;
                        if idx >= bytes.len() || bytes[idx] != b'[' {
                            return Err(PatternError::BadEscape {
                                offset: start,
                                ch: 'f',
                            });
                        }
                        let set_start = idx;
                        idx = scan_set(bytes, idx)?;
                        let _ = set_start;
                        prev_quantifiable = false;
                    }
                    // `%%` is a literal percent. Quantifiable.
                    b'%' => {
                        idx += 1;
                        prev_quantifiable = true;
                    }
                    // Any other punctuation after `%` is an escaped
                    // literal. Quantifiable.
                    ch if !ch.is_ascii_alphabetic() => {
                        idx += 1;
                        prev_quantifiable = true;
                    }
                    other => {
                        return Err(PatternError::BadEscape {
                            offset: idx,
                            ch: other as char,
                        });
                    }
                }
            }
            b'[' => {
                idx = scan_set(bytes, idx)?;
                prev_quantifiable = true;
            }
            b'(' => {
                idx += 1;
                open_captures += 1;
                captures += 1;
                prev_quantifiable = false;
            }
            b')' => {
                if open_captures == 0 {
                    return Err(PatternError::UnmatchedCapture { offset: idx });
                }
                open_captures -= 1;
                idx += 1;
                prev_quantifiable = true;
            }
            b'*' | b'+' | b'-' | b'?' => {
                if !prev_quantifiable {
                    return Err(PatternError::InvalidQuantifierTarget { offset: idx });
                }
                idx += 1;
                prev_quantifiable = false;
            }
            b'.' => {
                // The `.` class matches any character. Quantifiable.
                idx += 1;
                prev_quantifiable = true;
            }
            _ => {
                // Plain literal byte. Advance by UTF-8 char length so the
                // offset returned in error reports is on a char boundary.
                idx += utf8_char_len(bytes, idx);
                prev_quantifiable = true;
            }
        }
    }

    if open_captures != 0 {
        return Err(PatternError::UnmatchedCapture {
            offset: pattern.len(),
        });
    }

    Ok(captures)
}

/// Scan a `[set]` starting at `idx` (where `bytes[idx] == b'['`).
/// Returns the byte index just past the closing `]`.
fn scan_set(bytes: &[u8], mut idx: usize) -> Result<usize, PatternError> {
    let start = idx;
    debug_assert_eq!(bytes[idx], b'[');
    idx += 1;

    // Optional leading `^` negates the set; does not count toward content.
    if idx < bytes.len() && bytes[idx] == b'^' {
        idx += 1;
    }

    // Plain `[]` (or `[^]`) - closing bracket with no preceding content -
    // is rejected as an empty set. Lua's "leading `]` is literal" rule
    // only applies when at least one other character follows.
    if idx < bytes.len() && bytes[idx] == b']' {
        return Err(PatternError::EmptySet { offset: start });
    }

    while idx < bytes.len() {
        let here = bytes[idx];
        if here == b']' {
            return Ok(idx + 1);
        }
        if here == b'%' {
            idx += 1;
            if idx >= bytes.len() {
                return Err(PatternError::TruncatedSpecifier { offset: idx - 1 });
            }
            idx += 1;
            continue;
        }
        idx += utf8_char_len(bytes, idx);
    }

    Err(PatternError::UnterminatedSet { offset: start })
}

fn utf8_char_len(bytes: &[u8], idx: usize) -> usize {
    let lead = bytes[idx];
    if lead < 0x80 {
        1
    } else if lead < 0xC0 {
        // Continuation byte - shouldn't be a leading position, but be
        // defensive and step one byte.
        1
    } else if lead < 0xE0 {
        2
    } else if lead < 0xF0 {
        3
    } else {
        4
    }
    .min(bytes.len() - idx)
    .max(1)
}

/// Validate a `string.pack`/`string.unpack`/`string.packsize` format.
/// Returns the count of values the format consumes (i.e. options that
/// pack/unpack a value; control options like `<`, `>`, `=`, `!`, `x` do
/// not count).
pub fn validate_pack_format(pattern: &str) -> Result<usize, PatternError> {
    let bytes = pattern.as_bytes();
    let mut idx = 0;
    let mut values = 0;

    while idx < bytes.len() {
        let here = bytes[idx];

        // Whitespace is ignored between options.
        if here == b' ' || here == b'\t' {
            idx += 1;
            continue;
        }

        match here {
            // Endianness/alignment controls - no value consumed. The
            // `!` option takes an optional alignment size suffix.
            b'<' | b'>' | b'=' => {
                idx += 1;
            }
            b'!' => {
                idx += 1;
                while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
            }
            // Padding byte - emits one byte, consumes no value.
            b'x' => {
                idx += 1;
            }
            // Align-to-type. Reads a following type option but does not
            // itself consume a value. The next option must be a sized
            // type letter; we only check that one follows.
            b'X' => {
                idx += 1;
                while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
                    idx += 1;
                }
                if idx >= bytes.len() {
                    return Err(PatternError::TruncatedPackSize { offset: idx });
                }
                // Consume the argument option letter (and any digits
                // following it) so we don't double-count it as a value.
                let arg = bytes[idx];
                if !matches!(
                    arg,
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
                        offset: idx,
                        ch: arg as char,
                    });
                }
                idx += 1;
                while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
            }
            // Fixed-size integer types. `i`/`I` accept an optional size
            // suffix; `b`/`B`/`h`/`H`/`l`/`L`/`j`/`J`/`T` do not.
            b'b' | b'B' | b'h' | b'H' | b'l' | b'L' | b'j' | b'J' | b'T' => {
                idx += 1;
                values += 1;
            }
            b'i' | b'I' => {
                idx += 1;
                while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
                values += 1;
            }
            // Floats - no size suffix.
            b'f' | b'd' | b'n' => {
                idx += 1;
                values += 1;
            }
            // String options. `s` packs a length-prefixed string with an
            // optional size suffix; `z` packs a NUL-terminated string.
            b's' => {
                idx += 1;
                while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
                values += 1;
            }
            b'z' => {
                idx += 1;
                values += 1;
            }
            // Fixed-length string `c<n>` - requires a size suffix.
            b'c' => {
                idx += 1;
                let size_start = idx;
                while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
                if idx == size_start {
                    return Err(PatternError::TruncatedPackSize { offset: size_start });
                }
                values += 1;
            }
            // A digit that wasn't consumed by an option above is a stray
            // size suffix without an option - invalid.
            digit if digit.is_ascii_digit() => {
                return Err(PatternError::InvalidPackOption {
                    offset: idx,
                    ch: digit as char,
                });
            }
            other => {
                return Err(PatternError::InvalidPackOption {
                    offset: idx,
                    ch: other as char,
                });
            }
        }
    }

    Ok(values)
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
        assert!(matches!(
            validate_format("%"),
            Err(PatternError::TruncatedSpecifier { .. })
        ));
    }

    #[test]
    fn format_unknown_specifier() {
        // `%z` is valid in patterns but not in format specs.
        assert!(matches!(
            validate_format("%z"),
            Err(PatternError::UnknownSpecifier { ch: 'z', .. })
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
        assert!(matches!(
            validate_lua_pattern("[a-"),
            Err(PatternError::UnterminatedSet { .. })
        ));
    }

    #[test]
    fn pattern_empty_set() {
        assert!(matches!(
            validate_lua_pattern("[]"),
            Err(PatternError::EmptySet { .. })
        ));
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
            Err(PatternError::TruncatedSpecifier { .. })
                | Err(PatternError::BadEscape { ch: 'f', .. })
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
        assert!(matches!(
            validate_pack_format("X"),
            Err(PatternError::TruncatedPackSize { .. })
        ));
    }

    #[test]
    fn pack_size_on_unsized_option() {
        // `x` is a padding byte - it does not accept a size suffix. A
        // trailing digit is a stray size with no host option.
        assert!(matches!(
            validate_pack_format("x4"),
            Err(PatternError::InvalidPackOption { ch: '4', .. })
        ));
    }
}
