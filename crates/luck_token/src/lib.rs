//! # `luck_token`
//!
//! Foundation crate for the luck toolchain. Defines [`Span`] (byte ranges),
//! [`TokenKind`]/[`Token`] (every Luau token, which holds no text and slices
//! it from its source), the AST operators ([`BinOp`], [`UnOp`],
//! [`CompoundOp`]), [`Comment`] (extracted comments with position metadata),
//! the comments that tell a tool what to do ([`CommentContent`],
//! [`LintDirective`]), what a literal means and how to spell a value
//! ([`read_numeral`], [`read_escape`], [`decode_string`], [`spell_number`],
//! [`spell_string`]), the definition of a line break ([`line_breaks`],
//! [`has_line_break`]) with the [`LineIndex`] over it, what a chunk opens
//! with before its code ([`has_shebang`], [`leading_hot_comments`],
//! [`chunk_header`]), and Luau's compiler limits ([`MAX_ACTIVE_LOCALS`],
//! [`MAX_REGISTERS`]).
//!
//! A source is bytes, as Luau reads it, so its strings and comments need not
//! be UTF-8. Its names and numerals are ASCII.
//!
//! This crate depends on no other luck crate, and every luck crate that
//! reads or writes Luau builds on it.
//!
//! # Usage
//!
//! ```
//! use luck_token::{Span, Token, TokenKind};
//!
//! assert_eq!(TokenKind::keyword("continue"), Some(TokenKind::Continue));
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

mod chunk;
mod comment;
mod directive;
mod limits;
mod line_index;
mod number;
mod op;
mod span;
mod string;
mod token;
mod whitespace;

pub use chunk::{chunk_header, has_shebang, leading_hot_comments};
pub use comment::{Comment, CommentKind, CommentPosition};
pub use directive::{
    CommentContent, DirectiveError, DirectiveErrorKind, LintDirective, LintExtent, LintVerb,
};
pub use limits::{MAX_ACTIVE_LOCALS, MAX_REGISTERS};
pub use line_index::LineIndex;
pub use number::{NumberSpelling, Numeral, read_numeral, spell_number};
pub use op::{Associativity, BinOp, CompoundOp, UnOp};
pub use span::{Span, index_to_offset, offset_to_index};
pub use string::{
    EscapeError, Escaped, StringKind, StringSpelling, StringValue, decode_interpolated_text,
    decode_string, read_escape, spell_string, spell_string_shorter_than, write_interpolated_text,
    write_long_string,
};
pub use token::{Token, TokenKind};
pub use whitespace::{
    ends_with_line_break, has_line_break, is_luau_whitespace, line_breaks, strip_line_break,
};
