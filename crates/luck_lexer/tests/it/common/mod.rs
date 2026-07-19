use luck_lexer::{LexResult, lex};
use luck_token::{LuaVersion, TokenKind};

pub fn lex51(source: &str) -> LexResult {
    lex(source, LuaVersion::Lua51)
}

pub fn kinds_v(source: &str, version: LuaVersion) -> Vec<TokenKind> {
    let result = lex(source, version);
    assert!(
        result.errors.is_empty(),
        "unexpected errors: {:?}",
        result.errors
    );
    result
        .tokens
        .into_iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .map(|t| t.kind)
        .collect()
}

pub fn first_kind_v(source: &str, version: LuaVersion) -> TokenKind {
    let ks = kinds_v(source, version);
    assert!(!ks.is_empty(), "no tokens produced");
    ks.into_iter().next().expect("asserted non-empty above")
}

pub fn kinds(source: &str) -> Vec<TokenKind> {
    kinds_v(source, LuaVersion::Lua51)
}

pub fn first_kind(source: &str) -> TokenKind {
    first_kind_v(source, LuaVersion::Lua51)
}
