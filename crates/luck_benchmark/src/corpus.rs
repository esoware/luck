//! Deterministic bench inputs. Two generated files exercise the Lua 5.4
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

// Pinned to a commit so bench inputs are immutable; the files are mirrored
// third-party code kept out of this repo.
const CORPUS_URL_BASE: &str = "https://raw.githubusercontent.com/esoware/luck-bench-corpus/9188b121d704c7fb2d0b2cd29e891bff0e57c384";

fn fetch_corpus_file(file_name: &str) -> String {
    let cache_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
    let cache_path = cache_dir.join(file_name);
    if let Ok(text) = std::fs::read_to_string(&cache_path) {
        return text;
    }
    let url = format!("{CORPUS_URL_BASE}/{file_name}");
    let text = ureq::get(&url)
        .call()
        .unwrap_or_else(|error| panic!("failed to fetch {url}: {error}"))
        .body_mut()
        .read_to_string()
        .unwrap_or_else(|error| panic!("failed to read {url}: {error}"));
    let _ = std::fs::create_dir_all(&cache_dir);
    let _ = std::fs::write(&cache_path, &text);
    text
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
        // Real-world Roblox Luau: 13k lines of comment- and string-heavy
        // hand-written code (Infinite Yield admin script).
        TestFile {
            file_name: "infinite_yield.luau",
            source_text: fetch_corpus_file("infinite_yield.luau"),
            version: LuaVersion::Luau,
            target: LuaTarget::LuauRoblox,
        },
        // Adversarial: 2.2 MB of VM-obfuscated Luau on a single line.
        TestFile {
            file_name: "obfuscated_vm.luau",
            source_text: fetch_corpus_file("obfuscated_vm.luau"),
            version: LuaVersion::Luau,
            target: LuaTarget::LuauRoblox,
        },
    ]
}
