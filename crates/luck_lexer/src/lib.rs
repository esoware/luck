//! # luck_lexer
//!
//! Single-pass lexer for Lua 5.1-5.5 and Luau. Converts source text into a flat
//! [`Token`] stream plus a separate [`Comment`] array.
//!
//! Version-gated: binary literals (Luau), floor division (5.3+), hex floats (5.2+),
//! interpolated strings (Luau), and more are only recognized for their respective versions.
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//!
//! let result = luck_lexer::lex("local x = 42", LuaVersion::Lua54);
//! assert!(result.errors.is_empty());
//! ```

mod cursor;
mod lexer;
mod number;
mod search;
mod string;

use luck_token::{Comment, LuaVersion, Token};

#[derive(Debug)]
pub struct LexResult {
    pub tokens: Vec<Token>,
    pub comments: Vec<Comment>,
    pub errors: Vec<LexError>,
}

/// A lexer error with position and message.
pub type LexError = luck_token::SourceError;

// Error construction stays out of the hot lexing loops; #[cold] keeps
// these calls laid out off the fallthrough path.
#[cold]
#[inline(never)]
pub(crate) fn lex_error(span: luck_token::Span, message: impl Into<String>) -> LexError {
    LexError {
        span,
        message: message.into(),
    }
}

#[must_use]
pub fn lex(source: &str, version: LuaVersion) -> LexResult {
    // Spans are u32 (hard invariant 2); a larger input would silently
    // wrap every span past 4 GB, so refuse it up front.
    if source.len() > u32::MAX as usize {
        return LexResult {
            tokens: Vec::new(),
            comments: Vec::new(),
            errors: vec![LexError {
                span: luck_token::Span::new(0, 0),
                message: format!(
                    "input is {} bytes; the maximum supported file size is 4 GiB",
                    source.len()
                ),
            }],
        };
    }
    let mut lexer = lexer::Lexer::new(source, version);
    lexer.tokenize()
}
