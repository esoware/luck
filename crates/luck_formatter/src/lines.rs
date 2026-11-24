//! Where a source's lines start and end, and where it holds a blank line, as
//! the chunk's dialect breaks lines.

use luck_token::{LuaVersion, Span, index_to_offset, is_lua_whitespace, strip_line_break};

/// `offset`'s line's start, when only spaces and tabs come before `offset` on
/// it.
pub(crate) fn own_line_start(source: &[u8], offset: u32) -> Option<u32> {
    let i = offset as usize;
    let before = &source[..i];
    let indent_length = before.iter().rev().take_while(|b| matches!(b, b' ' | b'\t')).count();
    let line_start = i - indent_length;
    let is_line_start = line_start == 0 || matches!(before[line_start - 1], b'\n' | b'\r');
    is_line_start.then(|| index_to_offset(line_start))
}

pub(crate) fn after_line_break(source: &[u8], offset: u32, version: LuaVersion) -> u32 {
    let rest = &source[offset as usize..];
    let remaining = strip_line_break(rest, version).map_or(rest.len(), <[u8]>::len);
    index_to_offset(source.len() - remaining)
}

/// Whether only whitespace comes before `offset` on its line, as `version`
/// breaks lines.
pub(crate) fn starts_line(source: &[u8], offset: u32, version: LuaVersion) -> bool {
    let before = &source[..offset as usize];
    let is_line_space = |b: &&u8| {
        matches!(b, b' ' | b'\t' | 0x0B | 0x0C) || (**b == b'\r' && !version.has_lfcr_line_break())
    };
    before.iter().rev().find(|b| !is_line_space(b)).is_none_or(|b| matches!(b, b'\n' | b'\r'))
}

pub(crate) fn has_blank_line(source: &[u8], start: u32, end: u32, version: LuaVersion) -> bool {
    run_has_blank(Span::new(start, end).text(source), version)
}

/// Whether the whitespace run immediately before `anchor` contains a blank
/// line (the gap between a block opener and its first statement).
pub(crate) fn has_leading_edge_blank(source: &[u8], anchor: u32, version: LuaVersion) -> bool {
    let preceding = &source[..anchor as usize];
    let run_length = preceding.iter().rev().take_while(|&&b| is_lua_whitespace(b)).count();
    run_has_blank(&preceding[preceding.len() - run_length..], version)
}

/// Whether the whitespace and `;` run immediately after `end` contains a blank
/// line (the gap between the last statement and the block closer).
pub(crate) fn has_trailing_edge_blank(source: &[u8], end: u32, version: LuaVersion) -> bool {
    let following = &source[end as usize..];
    let run_length = following.iter().take_while(|&&b| is_lua_whitespace(b) || b == b';').count();
    run_has_blank(&following[..run_length], version)
}

/// Whether `text` holds a blank line: two line breaks with no `;` between them,
/// since a line holding only a `;` isn't blank.
///
/// The text is a gap between tokens, a few bytes as a rule, so one walk beats
/// searching it.
const fn run_has_blank(text: &[u8], version: LuaVersion) -> bool {
    let mut remaining = text;
    let mut has_break = false;
    while let Some((&b, rest)) = remaining.split_first() {
        if let Some(after_break) = strip_line_break(remaining, version) {
            if has_break {
                return true;
            }
            has_break = true;
            remaining = after_break;
        } else {
            has_break &= b != b';';
            remaining = rest;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use luck_token::line_breaks;

    use super::*;

    #[test]
    fn a_comment_starts_its_line_after_only_whitespace() {
        for (source, offset, version, expected) in [
            ("--", 0, LuaVersion::Lua54, true),
            ("x\n \t--", 4, LuaVersion::Lua54, true),
            ("x --", 2, LuaVersion::Lua54, false),
            ("x\r--", 2, LuaVersion::Lua54, true),
            // Luau reads a lone CR as a space.
            ("x\r--", 2, LuaVersion::Luau, false),
            ("x\r\n--", 3, LuaVersion::Luau, true),
        ] {
            assert_eq!(
                starts_line(source.as_bytes(), offset, version),
                expected,
                "{source:?} in {version:?}"
            );
        }
    }

    #[test]
    fn a_blank_line_is_two_breaks_with_no_semicolon_between() {
        // Every gap of up to six bytes from these, against the definition.
        let pieces = ['\n', '\r', ';', ' ', 'x'];
        let mut gaps = vec![String::new()];
        let mut longest = gaps.clone();
        for _ in 0..6 {
            longest = longest
                .iter()
                .flat_map(|gap| pieces.iter().map(move |piece| format!("{gap}{piece}")))
                .collect();
            gaps.extend(longest.iter().cloned());
        }
        for version in [LuaVersion::Lua54, LuaVersion::Luau] {
            for gap in &gaps {
                let expected = gap
                    .split(';')
                    .any(|segment| line_breaks(segment.as_bytes(), version).nth(1).is_some());
                assert_eq!(
                    run_has_blank(gap.as_bytes(), version),
                    expected,
                    "{gap:?} in {version:?}"
                );
            }
        }
    }
}
