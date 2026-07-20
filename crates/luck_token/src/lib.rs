//! # luck_token
//!
//! Foundation crate for the luck toolchain. Defines [`Span`] (byte ranges),
//! [`LuaVersion`] (version-gated feature flags), [`TokenKind`]/[`Token`] (all Lua/Luau
//! token types), and [`Comment`] (extracted comments with position metadata).
//!
//! This crate has zero internal dependencies - every other luck crate depends on it.
//!
//! # Usage
//!
//! ```
//! use luck_token::{LuaVersion, Span};
//!
//! assert!(LuaVersion::Lua54.has_goto());
//! assert_eq!(Span::new(0, 2).merge(Span::new(5, 7)), Span::new(0, 7));
//! ```

pub mod code_buffer;
pub mod comment;
pub mod literal;
pub mod span;
pub mod token;
pub mod version;

/// A source-level error with position and message. The single error type for
/// the toolchain: `LexError`, `ParseError`, and `FormatError` alias it, so
/// lexer, parser, and formatter failures flow through one rendering path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub span: Span,
    pub message: String,
}

pub use comment::{Comment, CommentKind, CommentPosition};
pub use compact_str::CompactString;
pub use literal::{LuaNumber, NumberSubtypes};
pub use span::Span;
pub use token::{Assoc, BinOp, CompoundOp, Token, TokenKind, UNARY_PRECEDENCE, UnOp};
pub use version::{LuaVersion, StdlibEnvironment};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_error_carries_span_and_message() {
        let error = SourceError {
            span: Span::new(4, 9),
            message: "unexpected token".to_string(),
        };
        assert_eq!(error.span, Span::new(4, 9));
        assert_eq!(error.message, "unexpected token");
        assert_eq!(error, error.clone());
    }

    #[test]
    fn compact_string_keeps_short_identifiers_inline() {
        // README contract: short strings live on the stack, not the heap.
        assert!(!CompactString::from("short_ident").is_heap_allocated());
        // A string well past the inline capacity must spill to the heap.
        assert!(CompactString::from("x".repeat(64)).is_heap_allocated());
    }
}
