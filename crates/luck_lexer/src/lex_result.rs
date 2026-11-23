use luck_diagnostics::Diagnostic;
use luck_token::{Comment, Span, Token};

#[derive(Debug, Default)]
pub struct LexResult {
    /// Every token, ending in `Eof` for any source [`Lexer::new`](crate::Lexer::new) accepts.
    pub tokens: Vec<Token>,
    pub comments: Vec<Comment>,
    /// Each an E008.
    pub errors: Vec<Diagnostic>,
    /// What [`Lexer::unterminated`](crate::Lexer::unterminated) gives once
    /// the source is lexed.
    pub unterminated: Vec<Span>,
    /// The span of each escape of a character Lua 5.1 and Luau define no
    /// escape for, which they read as the character, from its `\`, in source
    /// order.
    pub lenient_escapes: Vec<Span>,
}
