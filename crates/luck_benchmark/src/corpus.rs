//! Deterministic bench inputs, shaped like oxc's `TestFiles` so bench
//! bodies stay close to theirs. Two generated files exercise the Lua 5.4
//! and Luau grammars at identical seeds; the idiomatic fixtures supply a
//! small real-code sample.

use luck_core::types::LuaTarget;
use luck_token::LuaVersion;

pub struct TestFile {
    pub file_name: &'static str,
    pub source_text: String,
    pub version: LuaVersion,
    pub target: LuaTarget,
}

// Seeds and statement budget are fixed so numbers are comparable
// run to run.
fn generated(version: LuaVersion) -> String {
    let mut source = String::new();
    for seed in 0..40 {
        source.push_str(&luck_testgen::generate(seed, version, 60));
        source.push('\n');
    }
    source
}

fn idiomatic() -> String {
    [
        include_str!("../../../tests/fixtures/idiomatic/control_flow.lua"),
        include_str!("../../../tests/fixtures/idiomatic/module_pattern.lua"),
        include_str!("../../../tests/fixtures/idiomatic/oop_self.lua"),
    ]
    .join("\n")
}

#[must_use]
pub fn test_files() -> Vec<TestFile> {
    vec![
        TestFile {
            file_name: "gen_lua54.lua",
            source_text: generated(LuaVersion::Lua54),
            version: LuaVersion::Lua54,
            target: LuaTarget::Lua54,
        },
        TestFile {
            file_name: "gen_luau.luau",
            source_text: generated(LuaVersion::Luau),
            version: LuaVersion::Luau,
            target: LuaTarget::Luau,
        },
        TestFile {
            file_name: "idiomatic.lua",
            source_text: idiomatic(),
            version: LuaVersion::Lua54,
            target: LuaTarget::Lua54,
        },
    ]
}
