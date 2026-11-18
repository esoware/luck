use luck_token::CommentPosition;

use crate::span::GetSpan as _;
use crate::stmt::{Block, Statement};

/// Comments live outside the AST, keyed by `attached_to` (the anchor node's
/// span start), so this is plain data that only the formatter interprets.
///
/// The formatter adds `-- ` when `text` doesn't already start with `--`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntheticComment<'text> {
    pub attached_to: u32,
    pub text: &'text str,
    pub position: CommentPosition,
}

impl<'text> SyntheticComment<'text> {
    #[must_use]
    pub fn leading(stmt: &Statement<'_>, text: &'text str) -> Self {
        Self { attached_to: stmt.span().start, text, position: CommentPosition::Leading }
    }

    #[must_use]
    pub fn trailing(stmt: &Statement<'_>, text: &'text str) -> Self {
        Self { attached_to: stmt.span().start, text, position: CommentPosition::Trailing }
    }

    /// A comment printed inside `block` when it's empty, such as
    /// `-- unreachable`.
    #[must_use]
    pub const fn dangling(block: &Block<'_>, text: &'text str) -> Self {
        Self { attached_to: block.span.start, text, position: CommentPosition::Leading }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::Fixture;

    #[test]
    fn comment_helpers_anchor_on_stmts() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt = synth.local(&["x"], [synth.number("1")]);
        let leading = SyntheticComment::leading(&stmt, "before");
        let trailing = SyntheticComment::trailing(&stmt, "after");
        assert_eq!(leading.attached_to, stmt.span().start);
        assert_eq!(leading.position, CommentPosition::Leading);
        assert_eq!(trailing.attached_to, stmt.span().start);
        assert_eq!(trailing.position, CommentPosition::Trailing);

        let block = synth.block([], None);
        let dangling = SyntheticComment::dangling(&block, "unreachable");
        assert_eq!(dangling.attached_to, block.span.start);
        assert_eq!(dangling.position, CommentPosition::Leading);
    }
}
