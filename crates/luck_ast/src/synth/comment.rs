use luck_token::{CommentPosition, CompactString};

use crate::stmt::{Block, Statement};

/// Comments live outside the AST, keyed by `attached_to` (the anchor node's
/// span start), so this is plain data that only the formatter interprets.
///
/// The formatter adds `-- ` when `text` doesn't already start with `--`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticComment {
    pub attached_to: u32,
    pub text: CompactString,
    pub position: CommentPosition,
}

impl SyntheticComment {
    #[must_use]
    pub fn leading(stmt: &Statement, text: &str) -> Self {
        Self {
            attached_to: stmt.span().start,
            text: CompactString::from(text),
            position: CommentPosition::Leading,
        }
    }

    #[must_use]
    pub fn trailing(stmt: &Statement, text: &str) -> Self {
        Self {
            attached_to: stmt.span().start,
            text: CompactString::from(text),
            position: CommentPosition::Trailing,
        }
    }

    /// A comment printed inside `block` when it's empty, such as
    /// `-- unreachable`.
    #[must_use]
    pub fn dangling(block: &Block, text: &str) -> Self {
        Self {
            attached_to: block.span.start,
            text: CompactString::from(text),
            position: CommentPosition::Leading,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Synth;

    #[test]
    fn comment_helpers_anchor_on_stmts() {
        let synth = Synth::new();
        let stmt = synth.local(&["x"], vec![synth.number("1")]);
        let leading = SyntheticComment::leading(&stmt, "before");
        let trailing = SyntheticComment::trailing(&stmt, "after");
        assert_eq!(leading.attached_to, stmt.span().start);
        assert_eq!(leading.position, CommentPosition::Leading);
        assert_eq!(trailing.attached_to, stmt.span().start);
        assert_eq!(trailing.position, CommentPosition::Trailing);

        let block = synth.block(vec![], None);
        let dangling = SyntheticComment::dangling(&block, "unreachable");
        assert_eq!(dangling.attached_to, block.span.start);
        assert_eq!(dangling.position, CommentPosition::Leading);
    }
}
