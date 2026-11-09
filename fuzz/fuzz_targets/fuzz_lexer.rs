#![no_main]
//! The lexer must never panic; every failure is a `SourceError`.

use libfuzzer_sys::fuzz_target;
use luck_fuzz::decode_input;

fuzz_target!(|data: &[u8]| {
    let Some((source, version)) = decode_input(data) else {
        return;
    };
    let _ = luck_lexer::lex(source, version);
});
