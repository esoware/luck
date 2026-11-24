//! # `luck_lexer`
//!
//! Single-pass lexer for Lua 5.1-5.5 and Luau. Converts a source into a flat
//! [`Token`](luck_token::Token) stream plus a separate [`Comment`](luck_token::Comment) array.
//! A token holds no text, only its kind, span, and flags, so it slices its
//! bytes from the source with [`Token::text`](luck_token::Token::text).
//!
//! A source is bytes, as Lua reads it, so a string or a comment may hold
//! any byte, and a byte that isn't UTF-8 anywhere else is one error.
//!
//! Version-gated tokens, such as floor division (5.3+ and Luau), hex floats
//! (5.2+), Luau's binary literals and interpolated strings, and contextual
//! keywords such as Luau's `type`, are recognized only for the versions that
//! have them.
//!
//! # Usage
//!
//! ```
//! use luck_token::{LuaVersion, TokenKind};
//!
//! let source = b"local x = 42";
//! let result = luck_lexer::lex(source, LuaVersion::Lua54);
//! assert!(result.errors.is_empty());
//! let name = result.tokens[1];
//! assert_eq!((name.kind, name.text(source)), (TokenKind::Identifier, &b"x"[..]));
//! ```

mod cursor;
mod lex_result;
mod lexer;
mod number;
mod search;

pub use lex_result::LexResult;
pub use lexer::{Lexer, lex};
