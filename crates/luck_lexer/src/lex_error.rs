use luck_token::Span;

pub type LexError = luck_token::SourceError;

// Cold so error construction stays off the hot lexing path.
#[cold]
#[inline(never)]
pub(crate) fn lex_error(span: Span, message: impl Into<String>) -> LexError {
    LexError { span, message: message.into() }
}
