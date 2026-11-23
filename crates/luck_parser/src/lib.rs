//! # `luck_parser`
//!
//! Pratt expression parser with recursive descent statement parsing for Lua
//! 5.1-5.5 and Luau, which builds a `luck_ast` tree in a `luck_allocator`
//! arena.
//!
//! Every tree it returns, with errors or without, is at most
//! [`luck_ast::MAX_NESTING_DEPTH`] levels deep, measured as
//! [`luck_ast::block_nesting_depth`] measures it, so no consumer of the tree
//! needs more stack than [`luck_ast::MIN_STACK_BYTES`]. Nesting past the cap
//! is one error that ends the parse, as input over 4 GiB is. Error recovery
//! reports multiple parse errors in a single pass.
//!
//! A source is bytes, as Lua reads it, so a string or a comment may hold any
//! byte.
//!
//! The tree keeps the parentheses that change what they hold, and the parse
//! records every other pair in its [`Groupings`], as it keeps comments beside
//! the tree. A node spans the tokens it was read from, so one around an
//! operand the source grouped spans the operand's parentheses too.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_token::LuaVersion;
//!
//! let allocator = Allocator::new();
//! let result = luck_parser::parse(&allocator, b"local x = 1 + 2", LuaVersion::Lua54);
//! assert!(result.errors.is_empty());
//! assert_eq!(result.chunk.block.stmts.len(), 1);
//! ```

#![cfg_attr(
    test,
    expect(clippy::missing_panics_doc, reason = "a Panics section says nothing about a test")
)]

extern crate alloc;

mod groupings;
mod parse_result;
mod parser;
mod validate;

pub use groupings::Groupings;
pub use parse_result::ParseResult;
pub use parser::{parse, parse_with_tokens};
pub use validate::validate;
