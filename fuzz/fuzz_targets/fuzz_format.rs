#![no_main]
//! Hard invariants 7 and 8 on arbitrary parseable input:
//! format(format(x)) == format(x), and formatted output re-parses clean.

use libfuzzer_sys::fuzz_target;
use luck_formatter::FormatOptions;
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
    if !luck_parser::parse(source, version).errors.is_empty() {
        return;
    }

    let options = FormatOptions::default();
    let first = luck_formatter::format(source, version, &options);
    if !first.errors.is_empty() {
        return;
    }

    let reparsed = luck_parser::parse(&first.output, version);
    assert!(
        reparsed.errors.is_empty(),
        "formatted output failed to reparse (invariant 8)"
    );

    let second = luck_formatter::format(&first.output, version, &options);
    assert_eq!(
        first.output, second.output,
        "format not idempotent (invariant 7)"
    );
});
