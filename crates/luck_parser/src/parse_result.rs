use luck_ast::Block;
use luck_token::Comment;

pub type ParseError = luck_token::SourceError;

#[derive(Debug)]
pub struct ParseResult {
    pub block: Block,
    pub comments: Vec<Comment>,
    /// Every lex and parse error, sorted by start offset.
    pub errors: Vec<ParseError>,
    pub source: String,
    /// How many levels deep `block` nests, as the parser counted them toward
    /// [`luck_ast::MAX_NESTING_DEPTH`]. With no errors, that's never fewer
    /// than [`luck_ast::block_nesting_depth`] finds.
    pub nesting_depth: usize,
}
