#![no_main]
//! The lexer must never panic; every failure is a `SourceError`.

use libfuzzer_sys::fuzz_target;
use luck_token::LuaVersion;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let version = LuaVersion::ALL[data.len() % LuaVersion::ALL.len()];
    let _ = luck_lexer::lex(source, version);
});
