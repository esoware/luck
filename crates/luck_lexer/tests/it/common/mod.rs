use luck_lexer::{LexResult, lex};
use luck_token::TokenKind;

/// Every token's kind and text in `source`, which `result` lexed, but the
/// closing `Eof`'s, errors or not.
pub(crate) fn token_texts<'source>(
    result: &LexResult,
    source: &'source str,
) -> Vec<(TokenKind, &'source str)> {
    result
        .tokens
        .iter()
        .filter(|token| token.kind != TokenKind::Eof)
        .map(|token| (token.kind, str::from_utf8(token.text(source.as_bytes())).unwrap()))
        .collect()
}

/// Every token's kind and text but the closing `Eof`, after asserting
/// `source` lexes clean.
pub(crate) fn tokens(source: &str) -> Vec<(TokenKind, &str)> {
    let result = lex(source.as_bytes());
    assert!(result.errors.is_empty(), "unexpected errors: {:?}", result.errors);
    token_texts(&result, source)
}

/// Every token kind but the closing `Eof`, after asserting `source` lexes
/// clean.
pub(crate) fn kinds(source: &str) -> Vec<TokenKind> {
    tokens(source).into_iter().map(|(kind, _)| kind).collect()
}

pub(crate) fn first_kind(source: &str) -> TokenKind {
    kinds(source)[0]
}
