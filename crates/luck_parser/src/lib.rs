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

mod attributes;
mod expr;
mod luau;
mod parser;
mod stmt;
mod validate;

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

/// Scope-context checks real Lua performs at compile time but that need
/// extra AST walks: writes to const bindings, goto/label resolution, and
/// Luau's continue/until rule. NOT part of [`parse`] - transform
/// pipelines don't pay for it; diagnostic front ends (`luck check`)
/// opt in explicitly. Only meaningful on a clean parse; recovery ASTs
/// cascade misleading secondary errors.
#[must_use]
pub fn validate(block: &Block, version: LuaVersion) -> Vec<ParseError> {
    let mut errors = Vec::new();
    validate::validate(block, version, &mut errors);
    errors
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
    // A chunk must consume its whole input. `parse_block` returns at any
    // block boundary (a `return`, a stray `end`), and silently accepting
    // trailing statements would drop them from the AST - every downstream
    // consumer (minifier, formatter, bundler) would then silently discard
    // user code. Suppressed after an earlier error: recovery may already
    // have abandoned the tail, and the first error is the real one.
    if !parser.at_eof() && !parser.has_errors() {
        let span = parser.current_span();
        let message = format!("expected end of file, found {}", parser.peek());
        parser.error(span, message);
    }
    let (comments, errors) = parser.finish();
    ParseResult {
        block,
        comments,
        errors,
        source,
    }
}
