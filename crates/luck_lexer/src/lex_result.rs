use luck_token::{Comment, Token};

use crate::lex_error::LexError;

#[derive(Debug)]
pub struct LexResult {
    /// Every token, ending in `Eof` for any source [`Lexer::new`](crate::Lexer::new) accepts.
    pub tokens: Vec<Token>,
    pub comments: Vec<Comment>,
    pub errors: Vec<LexError>,
}
