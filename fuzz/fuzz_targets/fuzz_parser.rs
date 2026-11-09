#![no_main]
//! The parser must never panic or overflow the stack on any input;
//! every failure is a `SourceError` in `ParseResult::errors`.

use libfuzzer_sys::fuzz_target;
use luck_fuzz::decode_input;

fuzz_target!(|data: &[u8]| {
    let Some((source, version)) = decode_input(data) else {
        return;
    };
    let _ = luck_parser::parse(source, version);
});
