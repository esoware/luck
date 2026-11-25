//! Luau counts only LF as a line break (`Lexer::consumeAny`), so a lone CR is
//! plain whitespace and a CRLF is one break by virtue of its LF.

use std::ops::Range;

/// Whether `byte` is whitespace to the lexer: Luau's `isSpace`, which is C's
/// `isspace` in the C locale.
pub const fn is_luau_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

pub const fn strip_line_break(bytes: &[u8]) -> Option<&[u8]> {
    match bytes {
        [b'\r', b'\n', rest @ ..] | [b'\n', rest @ ..] => Some(rest),
        _ => None,
    }
}

/// Each line break in `bytes`, which spans the CR of a CRLF too.
pub fn line_breaks(bytes: &[u8]) -> impl Iterator<Item = Range<usize>> {
    memchr::memchr_iter(b'\n', bytes).map(move |lf| {
        let start = if bytes[..lf].ends_with(b"\r") { lf - 1 } else { lf };
        start..lf + 1
    })
}

pub fn has_line_break(bytes: &[u8]) -> bool {
    memchr::memchr(b'\n', bytes).is_some()
}

pub const fn ends_with_line_break(bytes: &[u8]) -> bool {
    matches!(bytes, [.., b'\n'])
}
