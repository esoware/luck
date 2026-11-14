//! # `luck_formatter`
//!
//! Prettier-style code formatter for Lua 5.1-5.5 and Luau.
//!
//! [`format_block()`] formats an AST directly, including one built in code
//! with no source text. It prints each leaf's text as the tree holds it,
//! unchecked, and returns an error only for a tree nested past
//! `luck_ast::MAX_NESTING_DEPTH`. [`format()`] and [`format_range`] parse
//! source text first, which also gets them blank-line preservation,
//! `-- luck: format off` regions, and comments placed by byte position.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//! use luck_formatter::FormatOptions;
//!
//! let result = luck_formatter::format("local x=1", LuaVersion::Lua54, &FormatOptions::default());
//! assert!(result.errors.is_empty());
//! assert!(result.output.contains("local x = 1"));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod ast_equivalence;
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
mod tokens;
mod verify;

pub use ast_equivalence::{AstDiff, blocks_equivalent};
pub use comments::Comments;
pub use format::{FormatError, FormatResult, format, format_block, format_range};
pub use format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, HexCase,
    IndentStyle, LineEndings, QuoteStyle, SpaceAfterFunction,
};
pub use verify::format_and_verify;
