#![no_main]
//! The lexer must never panic; every failure is a `SourceError`. The token
//! stream ends in `Eof`, and every token and comment slices the source
//! whole, as `Token::text` relies on.

use std::ops::Range;

use libfuzzer_sys::fuzz_target;
use luck_fuzz::decode_input;
use luck_token::{Span, TokenKind};

fuzz_target!(|data: &[u8]| {
    let Some((source, version)) = decode_input(data) else {
        return;
    };
    let lexed = luck_lexer::lex(source, version);
    assert!(
        lexed.tokens.last().is_some_and(|token| token.kind == TokenKind::Eof),
        "the token stream doesn't end in `Eof`"
    );
    let slices = |span: Span| source.get(Range::<usize>::from(span)).is_some();
    for token in &lexed.tokens {
        assert!(
            slices(token.span) && slices(token.text_span()),
            "{token:?} leaves the source or splits a character"
        );
    }
    for comment in &lexed.comments {
        assert!(slices(comment.span), "{comment:?} leaves the source or splits a character");
    }
});
