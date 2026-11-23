use luck_ast::{Chunk, TreeCounts};
use luck_token::{Comment, Span};

use crate::groupings::Groupings;

pub type ParseError = luck_token::SourceError;

/// Everything one parse found. The tree lives in the arena the parse was
/// given, and its names and literals borrow `source`, but for the string
/// values decoding rewrote, which live in the arena.
#[derive(Debug)]
pub struct ParseResult<'ast> {
    pub source: &'ast str,
    /// The tree, in the version the source was parsed as.
    pub chunk: Chunk<'ast>,
    pub comments: Vec<Comment>,
    /// The parentheses `chunk` leaves out, which only grouped what they hold.
    pub groupings: Groupings,
    /// Every lex and parse error, sorted by start offset.
    pub errors: Vec<ParseError>,
    /// What the lexer's [`luck_lexer::LexResult::lenient_escapes`] holds.
    pub lenient_escapes: Vec<Span>,
    /// How many levels deep the chunk's block nests, as the parser counted
    /// them toward [`luck_ast::MAX_NESTING_DEPTH`]. With no errors, that's
    /// never fewer than [`luck_ast::block_nesting_depth`] finds.
    pub nesting_depth: usize,
    /// What the chunk holds, for `luck_semantic::analyze` to size its tables
    /// from.
    pub counts: TreeCounts,
}
