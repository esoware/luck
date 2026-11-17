//! # `luck_formatter`
//!
//! Prettier-style code formatter for Lua 5.1-5.5 and Luau.
//!
//! [`format_block()`] formats an AST directly, including one built in code
//! with no source text. It prints each leaf's text as the tree holds it,
//! unchecked, and returns an error only for a tree nested past
//! `luck_ast::MAX_NESTING_DEPTH`. [`format()`] parses source text first, and
//! [`format_parsed`] and [`format_range`] take an existing parse. The parse
//! gets them blank-line preservation, `-- luck: format off` regions, and
//! comments placed by byte position.
//!
//! Every entry point takes the arena the caller owns: `format` parses into
//! it, and all of them put the little text they rewrite there, such as a
//! requoted string. Everything else the output holds is borrowed from the
//! tree or the source, never copied.
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
//!     luck_formatter::format(&allocator, "local x=1", LuaVersion::Lua54, &FormatOptions::default());
//! assert!(result.errors.is_empty());
//! assert_eq!(result.output, "local x = 1\n");
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
mod directive;
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
pub use format::{FormatError, FormatResult, format, format_block, format_parsed, format_range};
pub use format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, HexCase,
    IndentStyle, LineEndings, QuoteStyle, SpaceAfterFunction,
};
