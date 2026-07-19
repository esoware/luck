#![no_main]
//! The lexer must never panic; every failure is a `SourceError`.

use libfuzzer_sys::fuzz_target;
use luck_token::LuaVersion;

const VERSIONS: [LuaVersion; 6] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
    LuaVersion::Luau,
];

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let version = VERSIONS[data.len() % VERSIONS.len()];
    let _ = luck_lexer::lex(source, version);
});
