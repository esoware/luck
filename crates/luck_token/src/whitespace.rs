//! PUC Lua reads LF, CR, CRLF, and LFCR as one line break each (`inclinenumber`
//! in `llex.c`).
//!
//! Luau counts only LF (`Lexer::consumeAny`), so a lone CR is plain whitespace there and a CRLF
//! is one break by virtue of its LF.

use core::iter;
use core::ops::Range;

use crate::LuaVersion;

/// Whether `byte` is whitespace to the lexer: Lua's `isspace` in the C locale,
/// which Luau's `isSpace` matches.
#[must_use]
pub const fn is_lua_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

#[must_use]
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

pub fn line_breaks(text: &str, version: LuaVersion) -> impl Iterator<Item = Range<usize>> {
    let bytes = text.as_bytes();
    let mut position = 0usize;
    iter::from_fn(move || {
        let rest = bytes.get(position..)?;
        let offset = if version.has_lfcr_line_break() {
            memchr::memchr2(b'\n', b'\r', rest)
        } else {
            memchr::memchr(b'\n', rest)
        }?;
        let start = position.saturating_add(offset);
        let after_break = strip_line_break(rest.get(offset..)?, version)?;
        position = bytes.len().saturating_sub(after_break.len());
        // Luau's scan finds only the LF, but a CR before it is part of the
        // same break.
        let has_leading_cr =
            !version.has_lfcr_line_break() && bytes.get(start.saturating_sub(1)) == Some(&b'\r');
        Some(if has_leading_cr { start.saturating_sub(1) } else { start }..position)
    })
}

#[must_use]
pub fn has_line_break(text: &str, version: LuaVersion) -> bool {
    line_breaks(text, version).next().is_some()
}

#[must_use]
pub fn ends_with_line_break(text: &str, version: LuaVersion) -> bool {
    text.ends_with('\n') || (version.has_lfcr_line_break() && text.ends_with('\r'))
}

/// The byte offset `source`'s code starts at: past a UTF-8 BOM where
/// `version`'s loader skips one, else 0.
#[must_use]
pub fn chunk_start(source: &str, version: LuaVersion) -> usize {
    if version.has_bom_skip() && source.starts_with('\u{FEFF}') { '\u{FEFF}'.len_utf8() } else { 0 }
}

/// Whether `chunk`, the source from [`chunk_start`] on, opens with a first line
/// `version`'s loader skips.
#[must_use]
pub const fn has_shebang(chunk: &[u8], version: LuaVersion) -> bool {
    match chunk {
        [b'#', b'!', _, ..] => true,
        [b'#', ..] => version.has_bare_hash_shebang(),
        _ => false,
    }
}
