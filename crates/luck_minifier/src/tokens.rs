use luck_token::Span;
use luck_token::token::{Token, TokenKind};

pub fn default_span() -> Span {
    Span::default()
}

pub fn make_ident(name: &str) -> Token {
    Token::new(TokenKind::Identifier(name.into()), Span::default())
}
