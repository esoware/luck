//! Where a source's lines start and end, and where it holds a blank line.
//! Only an LF breaks a line, so a lone CR is a space, and a CR before an LF
//! is part of its break.

use luck_token::{Span, index_to_offset, is_luau_whitespace, strip_line_break};

/// `offset`'s line's start, when only spaces and tabs come before `offset`
/// on it.
pub(crate) fn own_line_start(source: &[u8], offset: u32) -> Option<u32> {
    let i = offset as usize;
    let before = &source[..i];
    let indent_length = before.iter().rev().take_while(|b| matches!(b, b' ' | b'\t')).count();
    let line_start = i - indent_length;
    let is_line_start = line_start == 0 || before[line_start - 1] == b'\n';
    is_line_start.then(|| index_to_offset(line_start))
}

pub(crate) fn after_line_break(source: &[u8], offset: u32) -> u32 {
    let rest = &source[offset as usize..];
    let remaining = strip_line_break(rest).map_or(rest.len(), <[u8]>::len);
    index_to_offset(source.len() - remaining)
}

/// Whether only whitespace comes before `offset` on its line.
pub(crate) fn starts_line(source: &[u8], offset: u32) -> bool {
    source[..offset as usize]
        .iter()
        .rev()
        .take_while(|&&b| b != b'\n')
        .all(|&b| is_luau_whitespace(b))
}

pub(crate) fn has_blank_line(source: &[u8], start: u32, end: u32) -> bool {
    run_has_blank(Span::new(start, end).text(source))
}

/// Whether the whitespace run immediately before `anchor` contains a blank
/// line (the gap between a block opener and its first statement).
pub(crate) fn has_leading_edge_blank(source: &[u8], anchor: u32) -> bool {
    let preceding = &source[..anchor as usize];
    let run_length = preceding.iter().rev().take_while(|&&b| is_luau_whitespace(b)).count();
    run_has_blank(&preceding[preceding.len() - run_length..])
}

/// Whether the whitespace and `;` run immediately after `end` contains a blank
/// line (the gap between the last statement and the block closer).
pub(crate) fn has_trailing_edge_blank(source: &[u8], end: u32) -> bool {
    let following = &source[end as usize..];
    let run_length = following.iter().take_while(|&&b| is_luau_whitespace(b) || b == b';').count();
    run_has_blank(&following[..run_length])
}

/// Whether `text` holds a blank line: two LFs with no `;` between them, since
/// a line holding only a `;` isn't blank.
///
/// The text is a gap between tokens, a few bytes as a rule, so one walk beats
/// searching it.
fn run_has_blank(text: &[u8]) -> bool {
    let mut has_break = false;
    for &b in text {
        match b {
            b'\n' if has_break => return true,
            b'\n' => has_break = true,
            b';' => has_break = false,
            _ => {}
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
        for (source, offset, expected) in [
            ("--", 0, true),
            ("x\n \t--", 4, true),
            ("x --", 2, false),
            // A lone CR is a space.
            ("x\r--", 2, false),
            ("x\r\n--", 3, true),
            ("x\n\r--", 3, true),
        ] {
            assert_eq!(starts_line(source.as_bytes(), offset), expected, "{source:?}");
        }
    }

    #[test]
    fn a_line_starts_after_a_line_break_and_before_its_indentation() {
        for (source, offset, expected) in [
            ("--", 0, Some(0)),
            ("x\n \t--", 4, Some(2)),
            ("x --", 2, None),
            // A lone CR is a space.
            ("x\r --", 3, None),
            ("x\r\n --", 4, Some(3)),
        ] {
            assert_eq!(own_line_start(source.as_bytes(), offset), expected, "{source:?}");
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
        for gap in &gaps {
            let expected =
                gap.split(';').any(|segment| line_breaks(segment.as_bytes()).nth(1).is_some());
            assert_eq!(run_has_blank(gap.as_bytes()), expected, "{gap:?}");
        }
    }
}
