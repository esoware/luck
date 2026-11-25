//! PUC Lua reads LF, CR, CRLF, and LFCR as one line break each (`inclinenumber`
//! in `llex.c`).
//!
//! Luau counts only LF (`Lexer::consumeAny`), so a lone CR is plain whitespace there and a CRLF
//! is one break by virtue of its LF.

use std::iter;
use std::ops::Range;

use crate::LuaVersion;

/// Whether `byte` is whitespace to the lexer: Lua's `isspace` in the C locale,
/// which Luau's `isSpace` matches.
pub const fn is_lua_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

pub const fn strip_line_break(bytes: &[u8], version: LuaVersion) -> Option<&[u8]> {
    match bytes {
        [b'\r', b'\n', rest @ ..] => Some(rest),
        [b'\n', b'\r', rest @ ..] | [b'\r', rest @ ..] if version.has_lfcr_line_break() => {
            Some(rest)
        }
        [b'\n', rest @ ..] => Some(rest),
        _ => None,
    }
}

#[expect(clippy::missing_panics_doc, reason = "the scan stops only at a line break's first byte")]
pub fn line_breaks(bytes: &[u8], version: LuaVersion) -> impl Iterator<Item = Range<usize>> {
    let mut position = 0;
    iter::from_fn(move || {
        let rest = &bytes[position..];
        let offset = if version.has_lfcr_line_break() {
            memchr::memchr2(b'\n', b'\r', rest)
        } else {
            memchr::memchr(b'\n', rest)
        }?;
        let start = position + offset;
        let after_break =
            strip_line_break(&rest[offset..], version).expect("the scan stopped at a line break");
        position = bytes.len() - after_break.len();
        // Luau's scan finds only the LF, but a CR before it is part of the
        // same break.
        let has_leading_cr = !version.has_lfcr_line_break() && bytes[..start].ends_with(b"\r");
        Some(if has_leading_cr { start - 1 } else { start }..position)
    })
}

pub fn has_line_break(bytes: &[u8], version: LuaVersion) -> bool {
    line_breaks(bytes, version).next().is_some()
}

pub const fn ends_with_line_break(bytes: &[u8], version: LuaVersion) -> bool {
    match bytes {
        [.., b'\n'] => true,
        [.., b'\r'] => version.has_lfcr_line_break(),
        _ => false,
    }
}
