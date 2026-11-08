use crate::Span;

/// A comment extracted during lexing, stored in a flat sorted array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment {
    /// Full span including delimiters (`--`, `--[[ ]]`, or the shebang's `#`).
    pub span: Span,
    /// Start offset of the token this comment is attached to: the next token
    /// for a leading comment, the previous one for a trailing comment.
    pub attached_to: u32,
    pub kind: CommentKind,
    pub position: CommentPosition,
    /// Whether a newline appears before this comment (since the last token).
    pub is_preceded_by_newline: bool,
    /// Whether this comment ends its line, with only spaces or tabs between it
    /// and the newline or end of file.
    pub is_followed_by_newline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// `-- single line comment` (to end of line).
    Line,
    /// `--[[ block comment ]]` that fits on a single line.
    SingleLineBlock,
    /// `--[[ block comment\n spanning lines ]]`.
    MultiLineBlock,
    /// A first line starting with `#`, such as `#!/usr/bin/env lua`.
    Shebang,
}

/// Where the comment sits relative to its attached token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentPosition {
    Leading,
    /// Comment appears after the attached token on the same line.
    Trailing,
}
