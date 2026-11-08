#![no_main]
//! On any input that parses, minify(minify(x)) == minify(x) and the
//! minified output re-parses clean.

use libfuzzer_sys::fuzz_target;
use luck_core::TransformConfig;
use luck_token::LuaVersion;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let version = LuaVersion::ALL[data.len() % LuaVersion::ALL.len()];
    if !luck_parser::parse(source, version).errors.is_empty() {
        return;
    }

    let config = TransformConfig::default();
    let Ok(first) = luck_minifier::minify(source, version, &config, "fuzz.lua") else {
        // Unreachable: `minify` only fails on a parse error, and this input
        // just parsed clean.
        return;
    };

    let reparsed = luck_parser::parse(&first, version);
    assert!(reparsed.errors.is_empty(), "minified output failed to reparse");

    let Ok(second) = luck_minifier::minify(&first, version, &config, "fuzz.lua") else {
        panic!("second minify pass errored on its own output");
    };
    assert_eq!(first, second, "minify not idempotent");
});
