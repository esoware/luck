//! # `luck_parser`
//!
//! Pratt expression parser with recursive descent statement parsing for
//! Luau, which builds a `luck_ast` tree in a `luck_allocator` arena.
//!
//! Every tree it returns, with errors or without, is at most
//! [`luck_ast::MAX_NESTING_DEPTH`] levels deep, measured as
//! [`luck_ast::block_nesting_depth`] measures it, so no consumer of the tree
//! needs more stack than [`luck_ast::MIN_STACK_BYTES`]. Nesting past the cap
//! is one error that ends the parse, as input over 4 GiB is. Error recovery
//! reports multiple parse errors in a single pass.
//!
//! A source is bytes, as Luau reads it, so a string or a comment may hold
//! any byte.
//!
//! A source is code unless the caller reads it with [`parse_definitions`] as
//! a definition file, which also takes Luau's `declare` statements, as
//! Luau's parser does with `allowDeclarationSyntax`.
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
//!
//! let allocator = Allocator::new();
//! let result = luck_parser::parse(&allocator, b"local x = 1 + 2");
//! assert!(result.errors.is_empty());
//! assert_eq!(result.chunk.stmts.len(), 1);
//! ```

mod groupings;
mod parse_result;
mod parser;

pub use groupings::Groupings;
pub use parse_result::ParseResult;
pub use parser::{parse, parse_definitions, parse_definitions_with_tokens, parse_with_tokens};
