//! # `luck_parser`
//!
//! Pratt expression parser with recursive descent statement parsing for Lua
//! 5.1-5.5 and Luau.
//!
//! Every tree it returns, with errors or without, nests no deeper than the
//! dialect's reference compiler parses, in the levels it counts, which
//! [`luck_ast::block_nesting_depth`] measures, so no consumer of the tree
//! needs more than a bounded stack. Nesting past that is one error that ends
//! the parse, as input over 4 GiB is. Error recovery reports multiple parse
//! errors in a single pass.
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

#![cfg_attr(
    test,
    expect(clippy::missing_panics_doc, reason = "a Panics section says nothing about a test")
)]

mod parse_result;
mod parser;
mod validate;

pub use parse_result::{ParseError, ParseResult};
pub use parser::parse;
pub use validate::validate;
