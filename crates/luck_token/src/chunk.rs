//! What a chunk opens with before its code: a first line its loader skips as
//! a shebang, and hot comments, which take effect only before the first
//! token.

use crate::{Comment, CommentContent, Span, is_luau_whitespace};

/// Whether `source` opens with a first line Luau's loader skips: a `#!` line
/// longer than those two bytes.
pub const fn has_shebang(source: &[u8]) -> bool {
    matches!(source, [b'#', b'!', _, ..])
}

/// The hot comments before `first_token_start`, the start of a chunk's first
/// token, which are the only ones Luau honours. `comments` must be sorted.
pub fn leading_hot_comments(
    comments: &[Comment],
    first_token_start: u32,
) -> impl Iterator<Item = &Comment> {
    comments
        .iter()
        .take_while(move |comment| comment.span.start < first_token_start)
        .filter(|comment| comment.content == CommentContent::HotComment)
}

/// The lines a chunk printed without its comments has to open with, each
/// ending in a line break: its shebang, then its hot comments, which take
/// effect only there.
pub fn chunk_header<HotComments>(
    source: &[u8],
    shebang: Option<Span>,
    hot_comments: HotComments,
) -> Vec<u8>
where
    HotComments: IntoIterator<Item = Span>,
{
    let mut header = Vec::new();
    if let Some(span) = shebang {
        let line = span.text(source);
        // A trailing CR would pair with this line break into a CRLF.
        let length = line.iter().rposition(|&b| b != b'\r').map_or(0, |last| last + 1);
        header.extend_from_slice(&line[..length]);
        header.push(b'\n');
    }
    for span in hot_comments {
        let text = span.text(source);
        let length = text.iter().rposition(|&b| !is_luau_whitespace(b)).map_or(0, |last| last + 1);
        header.extend_from_slice(&text[..length]);
        header.push(b'\n');
    }
    header
}
