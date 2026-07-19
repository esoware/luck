use crate::Span;

/// A comment extracted during lexing, stored in a flat sorted array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Full span including delimiters (`--` or `--[[ ]]` or `#!`)
    pub span: Span,
    /// Byte offset of the token this comment is attached to.
    /// For leading comments: start of the next token.
    /// For trailing comments: start of the preceding token.
    pub attached_to: u32,
    /// Line comment vs block comment vs shebang.
    pub kind: CommentKind,
    /// Whether this comment leads or trails its attached token.
    pub position: CommentPosition,
    /// Whether a newline appears before this comment (since the last token).
    pub preceded_by_newline: bool,
    /// Whether a newline appears after this comment (before the next token).
    pub followed_by_newline: bool,
}

/// The kind of comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// `-- single line comment` (to end of line)
    Line,
    /// `--[[ block comment ]]` that fits on a single line
    SingleLineBlock,
    /// `--[[ block comment\n spanning lines ]]`
    MultiLineBlock,
    /// `#!/usr/bin/env lua` on line 1
    Shebang,
}

/// Where the comment sits relative to its attached token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentPosition {
    /// Comment appears before the attached token.
    Leading,
    /// Comment appears after the attached token on the same line.
    Trailing,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_construction_and_equality() {
        let comment = Comment {
            span: Span::new(0, 10),
            attached_to: 12,
            kind: CommentKind::Line,
            position: CommentPosition::Leading,
            preceded_by_newline: true,
            followed_by_newline: false,
        };
        assert_eq!(comment.kind, CommentKind::Line);
        assert_eq!(comment.position, CommentPosition::Leading);
        assert_eq!(comment.attached_to, 12);
        assert_eq!(comment, comment.clone());
    }

    #[test]
    fn comment_kinds_are_distinct() {
        assert_ne!(CommentKind::Line, CommentKind::SingleLineBlock);
        assert_ne!(CommentKind::MultiLineBlock, CommentKind::Shebang);
        assert_ne!(CommentPosition::Leading, CommentPosition::Trailing);
    }
}
