//! # `luck_parser`
//!
//! Pratt expression parser with recursive descent statement parsing for Lua
//! 5.1-5.5 and Luau, which builds a `luck_ast` tree in a `luck_allocator`
//! arena.
//!
//! Every tree it returns, with errors or without, is at most
//! [`luck_ast::MAX_NESTING_DEPTH`] levels deep, measured as
//! [`luck_ast::block_nesting_depth`] measures it, so no consumer of the tree
//! needs more than a bounded stack. Nesting past the cap is one error that
//! ends the parse, as input over 4 GiB is. Error recovery reports multiple
//! parse errors in a single pass.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_token::LuaVersion;
//!
//! let allocator = Allocator::new();
//! let result = luck_parser::parse(&allocator, "local x = 1 + 2", LuaVersion::Lua54);
//! assert!(result.errors.is_empty());
//! assert_eq!(result.block.stmts.len(), 1);
//! ```

#![cfg_attr(
    test,
    expect(clippy::missing_panics_doc, reason = "a Panics section says nothing about a test")
)]

mod parse_result;
mod parser;
mod validate;

pub use parse_result::{ParseError, ParseResult};
pub use parser::{parse, parse_with_tokens};
pub use validate::validate;
