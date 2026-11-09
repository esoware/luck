use crate::Span;

/// `LexError`, `ParseError`, and `FormatError` alias it, so lexer, parser,
/// and formatter failures share one rendering path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub span: Span,
    pub message: String,
}
