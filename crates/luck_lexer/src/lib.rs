//! # `luck_lexer`
//!
//! Single-pass lexer for Lua 5.1-5.5 and Luau. Converts source text into a flat
//! [`Token`](luck_token::Token) stream plus a separate [`Comment`](luck_token::Comment) array.
//!
//! Version-gated tokens, such as floor division (5.3+ and Luau), hex floats
//! (5.2+), and Luau's binary literals and interpolated strings, are recognized
//! only for the versions that have them.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//!
//! let result = luck_lexer::lex("local x = 42", LuaVersion::Lua54);
//! assert!(result.errors.is_empty());
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod cursor;
mod lex_error;
mod lex_result;
mod lexer;
mod number;
mod search;
mod string;

pub use lex_error::LexError;
pub use lex_result::LexResult;
pub use lexer::{Lexer, lex};
