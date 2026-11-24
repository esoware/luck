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
