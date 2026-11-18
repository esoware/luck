use crate::{CommentContent, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment {
    /// Full span including delimiters (`--`, `--[[ ]]`, or the shebang's `#`).
    pub span: Span,
    /// Start offset of the token this comment is attached to: the next token
    /// for a leading comment, the previous one for a trailing comment.
    pub attached_to: u32,
    pub kind: CommentKind,
    pub position: CommentPosition,
    /// Whether this comment ends its line, with only spaces or tabs between it
    /// and the newline or end of file.
    pub is_followed_by_newline: bool,
    pub content: CommentContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    Line,
    SingleLineBlock,
    MultiLineBlock,
    Shebang,
}

/// A comment leads the next token unless no line break comes between it and
/// the token before it, which it trails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentPosition {
    Leading,
    Trailing,
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
    source: &str,
    shebang: Option<Span>,
    hot_comments: HotComments,
) -> String
where
    HotComments: IntoIterator<Item = Span>,
{
    let mut header = String::new();
    if let Some(span) = shebang {
        // A trailing CR would pair with this line break into a CRLF.
        header.push_str(span.text(source).trim_end_matches('\r'));
        header.push('\n');
    }
    for span in hot_comments {
        header.push_str(span.text(source).trim_end());
        header.push('\n');
    }
    header
}
