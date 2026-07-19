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
pub mod token;

/// Byte range in source text. Uses u32 (not usize) for compact storage - 4 GB file limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn len(&self) -> u32 {
        self.end - self.start
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }

    #[must_use]
    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// Spans are `u32` workspace-wide (4GB cap); diagnostic/ariadne boundaries want `usize`.
/// This is the single conversion point for that boundary.
impl From<Span> for std::ops::Range<usize> {
    fn from(span: Span) -> Self {
        span.start as usize..span.end as usize
    }
}

/// Lua language version. Determines which syntax is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LuaVersion {
    Lua51,
    Lua52,
    Lua53,
    Lua54,
    Lua55,
    Luau,
}

impl LuaVersion {
    /// Whether `goto` and labels are supported.
    #[must_use]
    pub fn has_goto(self) -> bool {
        matches!(self, Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55)
    }

    /// Whether `//` floor division is supported.
    #[must_use]
    pub fn has_floor_div(self) -> bool {
        !matches!(self, Self::Lua51 | Self::Lua52)
    }

    /// Whether bitwise operators (&, |, ~, >>, <<) are supported.
    #[must_use]
    pub fn has_bitwise_ops(self) -> bool {
        matches!(self, Self::Lua53 | Self::Lua54 | Self::Lua55)
    }

    /// Whether local variable attributes (`<const>`, `<close>`) are supported.
    #[must_use]
    pub fn has_attributes(self) -> bool {
        matches!(self, Self::Lua54 | Self::Lua55)
    }

    #[must_use]
    pub fn has_global(self) -> bool {
        matches!(self, Self::Lua55)
    }

    /// Whether numbers have distinct integer/float subtypes (Lua 5.3+).
    /// Observable via `math.type`, `tostring` (`1` vs `1.0`), and `//`.
    /// Luau keeps a single f64 number type like 5.1/5.2.
    #[must_use]
    pub fn has_integer_subtype(self) -> bool {
        matches!(self, Self::Lua53 | Self::Lua54 | Self::Lua55)
    }

    /// Whether an attribute may precede the first name in an attnamelist
    /// (`local <const> x`, `global <const> y`). Lua 5.5 syntax.
    #[must_use]
    pub fn has_leading_attributes(self) -> bool {
        matches!(self, Self::Lua55)
    }

    /// Whether `break` is only valid as the last statement of a block.
    /// Lua 5.1's grammar has `laststat ::= return | break`; Luau extends it
    /// with `continue` but keeps the last-statement restriction. 5.2+
    /// allows `break` anywhere.
    #[must_use]
    pub fn break_is_last_stat_only(self) -> bool {
        matches!(self, Self::Lua51 | Self::Luau)
    }

    /// Whether a call's `(` on a new line is an "ambiguous syntax"
    /// parse error (5.1 and Luau). 5.2+ parses it as a call.
    #[must_use]
    pub fn has_ambiguous_call_newline_error(self) -> bool {
        matches!(self, Self::Lua51 | Self::Luau)
    }

    /// Whether `;` is a valid empty statement (Lua 5.2+, NOT Luau).
    #[must_use]
    pub fn has_empty_statement(self) -> bool {
        matches!(self, Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55)
    }

    /// Whether undefined escape sequences are lexer errors. Lua 5.1
    /// treats any escaped non-digit character as that literal character
    /// (`"\m"` is `"m"`); 5.2+ and Luau reject them.
    #[must_use]
    pub fn has_strict_escapes(self) -> bool {
        !matches!(self, Self::Lua51)
    }

    /// Whether `\x` hex escape is supported in strings.
    #[must_use]
    pub fn has_hex_escape(self) -> bool {
        !matches!(self, Self::Lua51)
    }

    /// Whether `\z` whitespace-skip escape is supported in strings.
    #[must_use]
    pub fn has_whitespace_escape(self) -> bool {
        !matches!(self, Self::Lua51)
    }

    /// Whether `\u{XXXX}` unicode escape is supported in strings.
    #[must_use]
    pub fn has_unicode_escape(self) -> bool {
        matches!(self, Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau)
    }

    /// Whether hex float literals (0x1.Fp10) are supported.
    #[must_use]
    pub fn has_hex_floats(self) -> bool {
        matches!(self, Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55)
    }

    /// Whether binary literals (0b1010) are supported.
    #[must_use]
    pub fn has_binary_literals(self) -> bool {
        matches!(self, Self::Luau)
    }

    /// Whether underscore separators in numbers (1_000) are supported.
    #[must_use]
    pub fn has_underscore_separators(self) -> bool {
        matches!(self, Self::Luau)
    }

    #[must_use]
    pub fn is_luau(self) -> bool {
        matches!(self, Self::Luau)
    }

    /// Whether for-loop control variables are read-only (5.5 makes
    /// assigning to them a compile error; earlier versions allow it).
    #[must_use]
    pub fn has_const_for_variables(self) -> bool {
        matches!(self, Self::Lua55)
    }

    /// Whether named varargs (...name) are supported.
    #[must_use]
    pub fn has_named_varargs(self) -> bool {
        matches!(self, Self::Lua55)
    }

    #[must_use]
    pub fn has_continue(self) -> bool {
        matches!(self, Self::Luau)
    }

    /// Whether compound assignment operators (+=, -=, etc.) are supported.
    #[must_use]
    pub fn has_compound_assignment(self) -> bool {
        matches!(self, Self::Luau)
    }

    /// Whether interpolated strings (`` `text{expr}` ``) are supported.
    #[must_use]
    pub fn has_interpolated_strings(self) -> bool {
        matches!(self, Self::Luau)
    }

    /// Whether float `%` is computed as fmod plus a sign fix (5.3+).
    /// 5.1, 5.2, and Luau compute `a - floor(a/b)*b`, which loses
    /// precision at large magnitudes and the sign of zero results.
    #[must_use]
    pub fn has_fmod_float_modulo(self) -> bool {
        matches!(self, Self::Lua53 | Self::Lua54 | Self::Lua55)
    }

    /// Whether the float `%` sign fix compares operand signs directly
    /// (5.4+). 5.3 tests `fmod(a,b)*b < 0`, whose product can underflow
    /// to zero for subnormal operands and skip the fix.
    #[must_use]
    pub fn has_float_modulo_sign_compare(self) -> bool {
        matches!(self, Self::Lua54 | Self::Lua55)
    }
}

/// The standard-library environment a Luau program targets. Only meaningful
/// for Luau; vanilla Lua versions always use `Standalone`. Replaces the bare
/// `roblox: bool` that semantic analysis threaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StdlibEnvironment {
    #[default]
    Standalone,
    Roblox,
}

impl StdlibEnvironment {
    /// Whether this is the Roblox environment.
    #[must_use]
    pub fn is_roblox(self) -> bool {
        matches!(self, StdlibEnvironment::Roblox)
    }
}

/// A source-level error with position and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub span: Span,
    pub message: String,
}

pub use comment::{Comment, CommentKind, CommentPosition};
pub use compact_str::CompactString;
pub use token::{Assoc, BinOp, CompoundOp, Token, TokenKind, UNARY_PRECEDENCE, UnOp};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_basics() {
        let span = Span::new(0, 10);
        assert_eq!(span.len(), 10);
        assert!(!span.is_empty());
        assert!(Span::new(5, 5).is_empty());
    }

    #[test]
    fn span_merge() {
        let a = Span::new(5, 10);
        let b = Span::new(20, 30);
        let merged = a.merge(b);
        assert_eq!(merged, Span::new(5, 30));
    }

    #[test]
    fn version_feature_flags() {
        use LuaVersion::*;

        // has_goto: 5.2-5.5 only (not 5.1, not Luau)
        assert!(!Lua51.has_goto());
        assert!(Lua52.has_goto());
        assert!(Lua53.has_goto());
        assert!(Lua54.has_goto());
        assert!(Lua55.has_goto());
        assert!(!Luau.has_goto());

        // has_floor_div: 5.3+ and Luau
        assert!(!Lua51.has_floor_div());
        assert!(!Lua52.has_floor_div());
        assert!(Lua53.has_floor_div());
        assert!(Lua54.has_floor_div());
        assert!(Lua55.has_floor_div());
        assert!(Luau.has_floor_div());

        // has_bitwise_ops: 5.3-5.5 only
        assert!(!Lua51.has_bitwise_ops());
        assert!(!Lua52.has_bitwise_ops());
        assert!(Lua53.has_bitwise_ops());
        assert!(Lua54.has_bitwise_ops());
        assert!(Lua55.has_bitwise_ops());
        assert!(!Luau.has_bitwise_ops());

        // has_attributes: 5.4+ only
        assert!(!Lua51.has_attributes());
        assert!(!Lua52.has_attributes());
        assert!(!Lua53.has_attributes());
        assert!(Lua54.has_attributes());
        assert!(Lua55.has_attributes());
        assert!(!Luau.has_attributes());

        // has_global: 5.5 only
        assert!(!Lua51.has_global());
        assert!(!Lua52.has_global());
        assert!(!Lua53.has_global());
        assert!(!Lua54.has_global());
        assert!(Lua55.has_global());
        assert!(!Luau.has_global());

        // break_is_last_stat_only: 5.1 and Luau (laststat grammar)
        assert!(Lua51.break_is_last_stat_only());
        assert!(!Lua52.break_is_last_stat_only());
        assert!(Luau.break_is_last_stat_only());

        // has_leading_attributes: 5.5 only
        assert!(!Lua54.has_leading_attributes());
        assert!(Lua55.has_leading_attributes());
        assert!(!Luau.has_leading_attributes());

        // has_empty_statement: 5.2-5.5 (not 5.1, not Luau)
        assert!(!Lua51.has_empty_statement());
        assert!(Lua52.has_empty_statement());
        assert!(Lua53.has_empty_statement());
        assert!(Lua54.has_empty_statement());
        assert!(Lua55.has_empty_statement());
        assert!(!Luau.has_empty_statement());

        // has_hex_escape: everything except 5.1
        assert!(!Lua51.has_hex_escape());
        assert!(Lua52.has_hex_escape());
        assert!(Luau.has_hex_escape());

        // has_whitespace_escape: everything except 5.1
        assert!(!Lua51.has_whitespace_escape());
        assert!(Lua52.has_whitespace_escape());
        assert!(Luau.has_whitespace_escape());

        // has_unicode_escape: 5.3+ and Luau
        assert!(!Lua51.has_unicode_escape());
        assert!(!Lua52.has_unicode_escape());
        assert!(Lua53.has_unicode_escape());
        assert!(Lua54.has_unicode_escape());
        assert!(Lua55.has_unicode_escape());
        assert!(Luau.has_unicode_escape());

        // has_hex_floats: 5.2-5.5 (not 5.1, not Luau)
        assert!(!Lua51.has_hex_floats());
        assert!(Lua52.has_hex_floats());
        assert!(Lua53.has_hex_floats());
        assert!(Lua54.has_hex_floats());
        assert!(Lua55.has_hex_floats());
        assert!(!Luau.has_hex_floats());

        // Luau-only features
        assert!(Luau.has_binary_literals());
        assert!(!Lua54.has_binary_literals());
        assert!(Luau.has_underscore_separators());
        assert!(!Lua54.has_underscore_separators());
        assert!(Luau.is_luau());
        assert!(!Lua54.is_luau());
        assert!(Luau.has_continue());
        assert!(!Lua54.has_continue());
        assert!(Luau.has_compound_assignment());
        assert!(!Lua54.has_compound_assignment());
        assert!(Luau.has_interpolated_strings());
        assert!(!Lua54.has_interpolated_strings());

        // 5.5-only features
        assert!(Lua55.has_named_varargs());
        assert!(!Lua54.has_named_varargs());
    }

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
    fn stdlib_environment_default_and_roblox() {
        assert_eq!(StdlibEnvironment::default(), StdlibEnvironment::Standalone);
        assert!(!StdlibEnvironment::Standalone.is_roblox());
        assert!(StdlibEnvironment::Roblox.is_roblox());
    }

    #[test]
    fn span_conversion_to_usize_range() {
        let range: std::ops::Range<usize> = Span::new(2, 8).into();
        assert_eq!(range, 2..8);
    }

    #[test]
    fn compact_string_keeps_short_identifiers_inline() {
        // README contract: short strings live on the stack, not the heap.
        assert!(!CompactString::from("short_ident").is_heap_allocated());
        // A string well past the inline capacity must spill to the heap.
        assert!(CompactString::from("x".repeat(64)).is_heap_allocated());
    }
}
