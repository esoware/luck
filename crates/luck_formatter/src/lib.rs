//! # `luck_formatter`
//!
//! Prettier-style code formatter for Lua 5.1-5.5 and Luau.
//!
//! [`format_chunk()`] formats a tree directly, including one built in code
//! with no source text. It prints each literal as the tree spells it,
//! unchecked, or spells one built in code from its value, and returns an
//! error only for a tree nested past `luck_ast::MAX_NESTING_DEPTH`.
//! [`format()`] parses source text first, and [`format_parsed`] and
//! [`format_range`] take an existing parse. The parse gets them blank-line
//! preservation, `-- luck: format off` regions, the parentheses the source
//! grouped an expression in, and comments, each placed at the nearest node
//! boundary.
//!
//! Every entry point takes an arena the caller owns, and `format` parses into
//! it. All of them build their element stream there, with the few texts they
//! rewrite, such as a requoted string. Every other text is borrowed from the
//! tree or the source, never copied. An arena reused across files, as from an
//! `AllocatorPool`, saves each one the heap's large allocations.
//!
//! A source is bytes, as Lua reads it, and so is the output, which keeps a
//! comment or a string that isn't UTF-8 as written and is UTF-8 whenever the
//! source is.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_formatter::FormatOptions;
//! use luck_token::LuaVersion;
//!
//! let allocator = Allocator::new();
//! let result =
//!     luck_formatter::format(&allocator, b"local x=1", LuaVersion::Lua54, &FormatOptions::default());
//! assert!(result.errors.is_empty());
//! assert_eq!(result.output, b"local x = 1\n");
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod comments;
#[cfg(test)]
mod fixtures;
mod format;
mod format_block;
mod format_expr;
mod format_function;
mod format_options;
mod format_stmt;
mod format_table;
mod format_type;
mod ir;
mod numbers;
mod printer;
mod quotes;
mod sort_requires;

pub use comments::Comments;
pub use format::{FormatResult, format, format_chunk, format_parsed, format_range};
pub use format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, HexCase,
    IndentStyle, LineEndings, QuoteStyle, SpaceAfterFunction,
};
