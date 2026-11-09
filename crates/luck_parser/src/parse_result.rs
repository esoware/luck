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
}
