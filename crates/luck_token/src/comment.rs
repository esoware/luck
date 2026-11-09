use crate::{Span, is_lua_whitespace};

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

/// The `--!` hot comments in the comment run that opens `source`.
///
/// Luau honours them only before the first token. Other comments, block
/// comments included, don't end the run, and `comments` must be sorted.
pub fn leading_hot_comments<'comments>(
    comments: &'comments [Comment],
    source: &str,
) -> impl Iterator<Item = &'comments Comment> {
    let mut run_end = 0;
    comments
        .iter()
        .take_while(move |comment| {
            let gap = Span::new(run_end, comment.span.start).text(source);
            run_end = comment.span.end;
            gap.bytes().all(is_lua_whitespace)
        })
        .filter(move |comment| comment.span.text(source).starts_with("--!"))
}
