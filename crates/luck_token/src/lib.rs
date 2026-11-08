//! # `luck_token`
//!
//! Foundation crate for the luck toolchain. Defines [`Span`] (byte ranges),
//! [`LuaVersion`] (version-gated feature flags), [`TokenKind`]/[`Token`]
//! (every Lua and Luau token), the AST operators ([`BinOp`], [`UnOp`],
//! [`CompoundOp`]), [`Comment`] (extracted comments with position metadata),
//! [`SourceError`], the literal semantics every crate shares
//! ([`parse_lua_number`], [`decode_string`], [`encode_string`],
//! [`short_string_body`]), and the [`CodeBuffer`] output builder.
//!
//! This crate has zero internal dependencies, and every other luck crate
//! depends on it.
//!
//! # Usage
//!
//! ```
//! use luck_token::{LuaVersion, Span};
//!
//! assert!(LuaVersion::Lua54.has_goto());
//! assert_eq!(Span::new(0, 2).merge(Span::new(5, 7)), Span::new(0, 7));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod code_buffer;
mod comment;
mod number;
mod operator;
mod source_error;
mod span;
mod string;
mod token;
mod version;

pub use code_buffer::CodeBuffer;
pub use comment::{Comment, CommentKind, CommentPosition};
pub use compact_str::CompactString;
pub use number::{LuaNumber, NumberSubtypes, parse_lua_number};
pub use operator::{Associativity, BinOp, CompoundOp, OperandSide, UnOp};
pub use source_error::SourceError;
pub use span::{Span, index_to_offset, offset_to_index};
pub use string::{decode_string, encode_string, short_string_body};
pub use token::{Token, TokenKind};
pub use version::{LuaVersion, StdlibEnvironment};
