use luck_lexer::{LexResult, lex};
use luck_token::{LuaVersion, TokenKind};

pub(crate) fn lex_lua51(source: &str) -> LexResult {
    lex(source, LuaVersion::Lua51)
}

/// Every token kind but the closing `Eof`, after asserting `source` lexes
/// clean under `version`.
pub(crate) fn kinds_in(source: &str, version: LuaVersion) -> Vec<TokenKind> {
    let result = lex(source, version);
    assert!(result.errors.is_empty(), "unexpected errors: {:?}", result.errors);
    result
        .tokens
        .into_iter()
        .filter(|token| token.kind != TokenKind::Eof)
        .map(|token| token.kind)
        .collect()
}

pub(crate) fn first_kind_in(source: &str, version: LuaVersion) -> TokenKind {
    let mut token_kinds = kinds_in(source, version);
    assert!(!token_kinds.is_empty(), "no tokens produced");
    token_kinds.swap_remove(0)
}

pub(crate) fn kinds(source: &str) -> Vec<TokenKind> {
    kinds_in(source, LuaVersion::Lua51)
}

pub(crate) fn first_kind(source: &str) -> TokenKind {
    first_kind_in(source, LuaVersion::Lua51)
}
