//! What a chunk opens with before its code: a UTF-8 BOM its loader skips, a
//! first line its loader skips as a shebang, and Luau's hot comments, which
//! take effect only before the first token.

use crate::{Comment, CommentContent, LuaVersion, Span, is_lua_whitespace};

/// The byte offset `source`'s code starts at: past a UTF-8 BOM where
/// `version`'s loader skips one, else 0.
pub fn chunk_start(source: &[u8], version: LuaVersion) -> usize {
    const BOM: &[u8] = "\u{FEFF}".as_bytes();
    if version.has_bom_skip() && source.starts_with(BOM) { BOM.len() } else { 0 }
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
/// ending in a line break: its shebang, then its Luau hot comments, which
/// take effect only there.
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
        let length = text.iter().rposition(|&b| !is_lua_whitespace(b)).map_or(0, |last| last + 1);
        header.extend_from_slice(&text[..length]);
        header.push(b'\n');
    }
    header
}
