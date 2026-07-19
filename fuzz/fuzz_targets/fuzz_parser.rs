#![no_main]
//! The parser must never panic or overflow the stack on any input;
//! every failure is a `SourceError` in `ParseResult::errors`.

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
    let _ = luck_parser::parse(source, version);
});
