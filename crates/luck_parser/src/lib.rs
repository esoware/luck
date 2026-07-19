//! # luck_parser
//!
//! Pratt expression parser with recursive descent statement parsing for Lua 5.1-5.5 and Luau.
//!
//! Depth-limited to prevent stack overflow on pathological input. Supports error recovery
//! to report multiple parse errors in a single pass.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//!
//! let result = luck_parser::parse("local x = 1 + 2", LuaVersion::Lua54);
//! // result.block - the AST
//! // result.errors - any parse errors
//! assert!(result.errors.is_empty());
//! ```

mod expr;
mod luau;
mod parser;
mod stmt;

use luck_ast::Block;
use luck_token::{Comment, LuaVersion};

/// Result of parsing source code: the AST, extracted comments, and any errors.
#[derive(Debug)]
pub struct ParseResult {
    pub block: Block,
    pub comments: Vec<Comment>,
    pub errors: Vec<ParseError>,
    pub source: String,
}

/// A parse error with position and message.
pub type ParseError = luck_token::SourceError;

#[must_use]
pub fn parse(source: &str, version: LuaVersion) -> ParseResult {
    let lex_result = luck_lexer::lex(source, version);
    let mut parser = parser::Parser::new(lex_result.tokens, lex_result.comments, version, source);
    let block = parser.parse_block();
    let comments = std::mem::take(&mut parser.comments);
    let errors = parser.into_errors(lex_result.errors);
    ParseResult {
        block,
        comments,
        errors,
        source: source.to_string(),
    }
}
