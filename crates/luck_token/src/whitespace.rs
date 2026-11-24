//! PUC Lua reads LF, CR, CRLF, and LFCR as one line break each (`inclinenumber`
//! in `llex.c`).
//!
//! Luau counts only LF (`Lexer::consumeAny`), so a lone CR is plain whitespace there and a CRLF
//! is one break by virtue of its LF.

use std::iter;
use std::ops::Range;

use crate::{LuaVersion, index_to_offset};

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

/// The byte offset `source`'s code starts at: past a UTF-8 BOM where
/// `version`'s loader skips one, else 0.
pub fn chunk_start(source: &[u8], version: LuaVersion) -> usize {
    const BOM: &[u8] = "\u{FEFF}".as_bytes();
    if version.has_bom_skip() && source.starts_with(BOM) { BOM.len() } else { 0 }
}

/// Where each line of a source starts, with lines broken as `version` breaks
/// them and the first one starting past a BOM its loader skips.
#[derive(Debug, Clone)]
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(source: &[u8], version: LuaVersion) -> Self {
        let first_line_start = chunk_start(source, version);
        let chunk = &source[first_line_start..];
        // Loaders skip a shebang line up to its LF, so a CR in it starts no
        // line.
        let shebang_length = if has_shebang(chunk, version) {
            memchr::memchr(b'\n', chunk).unwrap_or(chunk.len())
        } else {
            0
        };
        let scan_start = first_line_start + shebang_length;
        let rest = &source[scan_start..];
        let line_starts = iter::once(index_to_offset(first_line_start))
            .chain(
                line_breaks(rest, version)
                    .map(|line_break| index_to_offset(scan_start + line_break.end)),
            )
            .collect();
        Self { line_starts }
    }

    /// The one-based line `offset` is on.
    pub fn line(&self, offset: u32) -> u32 {
        index_to_offset(self.line_starts.partition_point(|&start| start <= offset)).max(1)
    }

    /// The one-based line and byte column of `offset`.
    pub fn line_column(&self, offset: u32) -> (u32, u32) {
        let line = self.line(offset);
        let line_start = self.line_starts[(line - 1) as usize];
        // An offset in a BOM before the first line is in its first column.
        (line, offset.saturating_sub(line_start) + 1)
    }
}

/// Whether `chunk`, the source from [`chunk_start`] on, opens with a first line
/// `version`'s loader skips.
pub const fn has_shebang(chunk: &[u8], version: LuaVersion) -> bool {
    match chunk {
        [b'#', b'!', _, ..] => true,
        [b'#', ..] => version.has_bare_hash_shebang(),
        _ => false,
    }
}
