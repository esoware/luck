#![no_main]
//! The parser must never panic or overflow the stack on any input; every
//! failure is a `SourceError` in `ParseResult::errors`. The tree stays within
//! the nesting cap every traversal's stack depends on, errors or not, and the
//! parse reads the token stream and comments `lex` gives.

use libfuzzer_sys::fuzz_target;
use luck_ast::{MAX_NESTING_DEPTH, block_nesting_depth};
use luck_fuzz::{decode_input, with_arena};

fuzz_target!(|data: &[u8]| {
    let Some((source, version)) = decode_input(data) else {
        return;
    };
    with_arena(|allocator| {
        let (parse, tokens) = luck_parser::parse_with_tokens(allocator, source, version);
        assert!(
            block_nesting_depth(&parse.block) <= MAX_NESTING_DEPTH,
            "the tree nests past the cap"
        );
        let lexed = luck_lexer::lex(source, version);
        assert!(tokens == lexed.tokens, "the parse read tokens `lex` doesn't give");
        assert!(parse.comments == lexed.comments, "the parse found comments `lex` doesn't");
    });
});
