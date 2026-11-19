//! # `luck_token`
//!
//! Foundation crate for the luck toolchain. Defines [`Span`] (byte ranges),
//! [`LuaVersion`] (version-gated feature flags), [`TokenKind`]/[`Token`]
//! (every Lua and Luau token, which holds no text and slices it from its
//! source), the AST operators ([`BinOp`], [`UnOp`], [`CompoundOp`]),
//! [`Comment`] (extracted comments with position metadata), the comments that
//! tell a tool what to do ([`CommentContent`], [`LintDirective`]), [`SourceError`],
//! what a literal means and how to spell a value ([`read_numeral`],
//! [`read_escape`], [`decode_string`], [`spell_number`], [`spell_string`]),
//! the definition of a line break ([`line_breaks`], [`has_line_break`]) and
//! the [`LineIndex`] over it, and the [`CodeBuffer`] output builder.
//!
//! This crate has zero internal dependencies, and every luck crate but
//! `luck_allocator` builds on it.
//!
//! # Usage
//!
//! ```
//! use luck_token::{LuaVersion, Span, Token, TokenKind};
//!
//! assert!(LuaVersion::Lua54.has_goto());
//! assert_eq!(Span::new(0, 2).merge(Span::new(5, 7)), Span::new(0, 7));
//!
//! let source = "`hello {name}`";
//! let begin = Token {
//!     span: Span::new(0, 8),
//!     kind: TokenKind::InterpolatedStringBegin,
//!     has_line_break_before: true,
//!     is_unterminated: false,
//! };
//! assert_eq!(begin.text(source), "hello ");
//! ```

mod code_buffer;
mod comment;
mod directive;
mod number;
mod op;
mod source_error;
mod span;
mod string;
mod token;
mod version;
mod whitespace;

pub use code_buffer::CodeBuffer;
pub use comment::{Comment, CommentKind, CommentPosition, chunk_header, leading_hot_comments};
pub use directive::{CommentContent, LintDirective, LintExtent, LintVerb};
pub use number::{
    LuaNumber, NumberSpelling, Numeral, read_integer, read_numeral, spell_integer, spell_number,
};
pub use op::{Associativity, BinOp, CompoundOp, OperandSide, UnOp};
pub use source_error::SourceError;
pub use span::{Span, index_to_offset, offset_to_index};
pub use string::{
    EscapeError, Escaped, StringSpelling, StringValue, decode_interpolated_text, decode_string,
    read_escape, spell_string, write_interpolated_text, write_long_string,
};
pub use token::{Token, TokenKind};
pub use version::{LuaVersion, MAX_ACTIVE_LOCALS, StdlibEnvironment};
pub use whitespace::{
    LineIndex, chunk_start, ends_with_line_break, has_line_break, has_shebang, is_lua_whitespace,
    line_breaks, strip_line_break,
};
