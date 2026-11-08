#![no_main]
//! The parser must never panic or overflow the stack on any input;
//! every failure is a `SourceError` in `ParseResult::errors`.

use libfuzzer_sys::fuzz_target;
use luck_token::LuaVersion;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let version = LuaVersion::ALL[data.len() % LuaVersion::ALL.len()];
    let _ = luck_parser::parse(source, version);
});
