#![no_main]
//! Hard invariants 7 and 8 for the minifier on arbitrary parseable input.

use libfuzzer_sys::fuzz_target;
use luck_core::transform_config::TransformConfig;
use luck_core::types::LuaTarget;
use luck_token::LuaVersion;

const TARGETS: [(LuaVersion, LuaTarget); 6] = [
    (LuaVersion::Lua51, LuaTarget::Lua51),
    (LuaVersion::Lua52, LuaTarget::Lua52),
    (LuaVersion::Lua53, LuaTarget::Lua53),
    (LuaVersion::Lua54, LuaTarget::Lua54),
    (LuaVersion::Lua55, LuaTarget::Lua55),
    (LuaVersion::Luau, LuaTarget::Luau),
];

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let (version, target) = TARGETS[data.len() % TARGETS.len()];
    if !luck_parser::parse(source, version).errors.is_empty() {
        return;
    }

    let config = TransformConfig::default();
    let Ok(first) = luck_minifier::minify(source, target, &config, "fuzz.lua") else {
        // Parse was clean, so minify erroring is itself suspect — but
        // some inputs legitimately hit resource guards; don't assert.
        return;
    };

    let reparsed = luck_parser::parse(&first, version);
    assert!(
        reparsed.errors.is_empty(),
        "minified output failed to reparse (invariant 8)"
    );

    let Ok(second) = luck_minifier::minify(&first, target, &config, "fuzz.lua") else {
        panic!("second minify pass errored on its own output");
    };
    assert_eq!(first, second, "minify not idempotent (invariant 7)");
});
