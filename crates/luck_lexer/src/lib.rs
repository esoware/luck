//! # `luck_lexer`
//!
//! Single-pass lexer for Luau, with SIMD on `x86_64` CPUs with AVX2 and on
//! aarch64. Converts a source into a flat [`Token`](luck_token::Token) stream
//! plus a separate [`Comment`](luck_token::Comment) array. A token holds no
//! text, only its kind, span, and flags, so it slices its bytes from the
//! source with [`Token::text`](luck_token::Token::text).
//!
//! A source is bytes, as Luau reads it, so a string or a comment may hold
//! any byte, and a byte that isn't UTF-8 anywhere else is one error.
//!
//! # Usage
//!
//! ```
//! use luck_token::TokenKind;
//!
//! let source = b"local x = 42";
//! let result = luck_lexer::lex(source);
//! assert!(result.errors.is_empty());
//! let name = result.tokens[1];
//! assert_eq!((name.kind, name.text(source)), (TokenKind::Identifier, &b"x"[..]));
//! ```

mod cursor;
mod lex_result;
mod lexer;
mod search;
mod window;

pub use lex_result::LexResult;
pub use lexer::{Lexer, lex};
