//! # `luck_token`
//!
//! Foundation crate for the luck toolchain. Defines [`Span`] (byte ranges),
//! [`LuaVersion`] (version-gated feature flags), [`LuaTarget`] (the dialect
//! a user names, a version and its host), [`TokenKind`]/[`Token`]
//! (every Lua and Luau token, which holds no text and slices it from its
//! source), the AST operators ([`BinOp`], [`UnOp`], [`CompoundOp`]),
//! [`Comment`] (extracted comments with position metadata), the comments that
//! tell a tool what to do ([`CommentContent`], [`LintDirective`]), what a
//! literal means and how to spell a value ([`read_numeral`], [`read_escape`],
//! [`decode_string`], [`spell_number`], [`spell_string`]), and the definition
//! of a line break ([`line_breaks`], [`has_line_break`]) with the
//! [`LineIndex`] over it.
//!
//! A source is bytes, as PUC Lua and Luau read it, so its strings and
//! comments need not be UTF-8. Its names and numerals are ASCII.
//!
//! This crate has zero internal dependencies, and every luck crate but
//! `luck_allocator` and `luck_data_structures` builds on it.
//!
//! # Usage
//!
//! ```
//! use luck_token::{LuaVersion, Span, Token, TokenKind};
//!
//! assert!(LuaVersion::Lua54.has_goto());
//! assert_eq!(Span::new(0, 2).merge(Span::new(5, 7)), Span::new(0, 7));
//!
//! let source = b"`hello {name}`";
//! let begin = Token {
//!     span: Span::new(0, 8),
//!     kind: TokenKind::InterpolatedStringBegin,
//!     has_line_break_before: true,
//!     is_unterminated: false,
//!     has_escape: false,
//! };
//! assert_eq!(begin.text(source), b"hello ");
//! ```

mod comment;
mod directive;
mod lua_target;
mod number;
mod op;
mod span;
mod string;
mod token;
mod version;
mod whitespace;

pub use comment::{Comment, CommentKind, CommentPosition, chunk_header, leading_hot_comments};
pub use directive::{
    CommentContent, DirectiveError, DirectiveErrorKind, LintDirective, LintExtent, LintVerb,
};
pub use lua_target::LuaTarget;
pub use number::{
    LuaNumber, NumberSpelling, Numeral, read_integer, read_numeral, spell_integer, spell_number,
};
pub use op::{Associativity, BinOp, CompoundOp, UnOp};
pub use span::{Span, index_to_offset, offset_to_index};
pub use string::{
    EscapeError, Escaped, StringSpelling, StringValue, decode_interpolated_text, decode_string,
    read_escape, spell_string, spell_string_shorter_than, write_interpolated_text,
    write_long_string,
};
pub use token::{Token, TokenKind};
pub use version::{LuaVersion, MAX_ACTIVE_LOCALS};
pub use whitespace::{
    LineIndex, chunk_start, ends_with_line_break, has_line_break, has_shebang, is_lua_whitespace,
    line_breaks, strip_line_break,
};
