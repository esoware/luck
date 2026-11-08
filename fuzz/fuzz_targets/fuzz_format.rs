#![no_main]
//! On any input that parses, format(format(x)) == format(x) and the
//! formatted output re-parses clean.

use libfuzzer_sys::fuzz_target;
use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let version = LuaVersion::ALL[data.len() % LuaVersion::ALL.len()];
    if !luck_parser::parse(source, version).errors.is_empty() {
        return;
    }

    let options = FormatOptions::default();
    let first = luck_formatter::format(source, version, &options);
    if !first.errors.is_empty() {
        return;
    }

    let reparsed = luck_parser::parse(&first.output, version);
    assert!(reparsed.errors.is_empty(), "formatted output failed to reparse");

    let second = luck_formatter::format(&first.output, version, &options);
    assert_eq!(first.output, second.output, "format not idempotent on its own output");
});
