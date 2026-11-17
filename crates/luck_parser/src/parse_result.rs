use luck_ast::{Block, TreeCounts};
use luck_token::Comment;

pub type ParseError = luck_token::SourceError;

/// Everything one parse found. The tree lives in the arena the parse was
/// given, and its names and literals borrow `source`.
#[derive(Debug)]
pub struct ParseResult<'ast> {
    pub source: &'ast str,
    pub block: Block<'ast>,
    pub comments: Vec<Comment>,
    /// Every lex and parse error, sorted by start offset.
    pub errors: Vec<ParseError>,
    /// How many levels deep `block` nests, as the parser counted them toward
    /// [`luck_ast::MAX_NESTING_DEPTH`]. With no errors, that's never fewer
    /// than [`luck_ast::block_nesting_depth`] finds.
    pub nesting_depth: usize,
    /// What `block` holds, for `luck_semantic::analyze` to size its tables
    /// from.
    pub counts: TreeCounts,
}
