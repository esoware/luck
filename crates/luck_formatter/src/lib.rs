//! # `luck_formatter`
//!
//! Prettier-style code formatter for Luau.
//!
//! [`format_chunk()`] formats a tree directly, including one built in code
//! with no source text. It prints each literal as the tree spells it,
//! unchecked, or spells one built in code from its value, and returns an
//! error only for a tree nested past `luck_ast::MAX_NESTING_DEPTH`.
//! [`format()`] parses source text first, as code, and [`format_parsed`]
//! and [`format_range`] take an existing parse, one that kept its tokens,
//! such as a definition file's from
//! `luck_parser::parse_definitions_with_tokens`. The parse gets them
//! blank-line preservation, `-- luck: format off` regions, the
//! parentheses the source grouped an expression in, and comments, each placed
//! at the nearest node boundary, where its tokens say what lies between two
//! nodes, so the formatter never lexes the source again.
//!
//! Every entry point takes an arena the caller owns, and `format` parses into
//! it. All of them build their element stream there, with the few texts they
//! rewrite, such as a requoted string. Every other text is borrowed from the
//! tree or the source, never copied. An arena reused across files, as from an
//! `AllocatorPool`, saves each one the heap's large allocations.
//!
//! A source is bytes, as Luau reads it, and so is the output, which keeps a
//! comment or a string that isn't UTF-8 as written and is UTF-8 whenever the
//! source is.
//!
//! [`FormatConfig`] is `luck.json`'s `format` section, each option unset
//! unless it's set, which layers over another and fills in the defaults of
//! [`FormatOptions`].
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_formatter::FormatOptions;
//!
//! let allocator = Allocator::new();
//! let result = luck_formatter::format(&allocator, b"local x=1", &FormatOptions::default());
//! assert!(result.errors.is_empty());
//! assert_eq!(result.output, b"local x = 1\n");
//! ```

mod comments;
#[cfg(test)]
mod fixtures;
mod format;
mod format_block;
mod format_comments;
mod format_config;
mod format_expr;
mod format_function;
mod format_options;
mod format_stmt;
mod format_table;
mod format_type;
mod ir;
mod lines;
mod numbers;
mod printer;
mod quotes;
mod sort_requires;

pub use comments::{Comments, SyntheticComment};
pub use format::{FormatResult, format, format_chunk, format_parsed, format_range};
pub use format_config::FormatConfig;
pub use format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, HexCase,
    IndentStyle, LineEndings, QuoteStyle, SpaceAfterFunction,
};
