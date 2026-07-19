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

/// Callers that own their source `String` should pass it by value: the
/// text is stored in `ParseResult.source` without a copy. Borrowed
/// `&str` input is copied once, as before.
#[must_use]
pub fn parse(source: impl Into<String>, version: LuaVersion) -> ParseResult {
    parse_owned(source.into(), version)
}

fn parse_owned(source: String, version: LuaVersion) -> ParseResult {
    // Spans are u32 (hard invariant 2); a larger input would silently
    // wrap every span past 4 GB, so refuse it up front.
    if source.len() > u32::MAX as usize {
        return ParseResult {
            block: Block {
                span: luck_token::Span::new(0, 0),
                stmts: Vec::new(),
                last_stmt: None,
            },
            comments: Vec::new(),
            errors: vec![ParseError {
                span: luck_token::Span::new(0, 0),
                message: format!(
                    "input is {} bytes; the maximum supported file size is 4 GiB",
                    source.len()
                ),
            }],
            source,
        };
    }
    let mut parser = parser::Parser::new(&source, version);
    let block = parser.parse_block();
    let (comments, errors) = parser.finish();
    ParseResult {
        block,
        comments,
        errors,
        source,
    }
}
