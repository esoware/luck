//! Deterministic bench inputs, three kinds:
//!
//! - two generated files from the full-grammar profile of `luck_testgen`
//!   (every version-gated construct, realistic statement mix) at fixed
//!   seeds, plus the idiomatic fixtures as a small hand-written sample;
//! - two real-world single files mirrored in esoware/luck-bench-corpus;
//! - two real multi-file projects fetched as pinned upstream tarballs:
//!   Roact (Roblox Luau, ~80 files) and Penlight (Lua 5.1, ~40 files).
//!
//! Everything network-fetched lands in the gitignored `corpus/` cache and
//! is pinned to a commit SHA, so inputs are immutable run to run.

use std::path::{Path, PathBuf};

use luck_core::types::LuaTarget;
use luck_token::LuaVersion;

pub struct TestFile {
    pub file_name: &'static str,
    pub source_text: String,
    pub version: LuaVersion,
    pub target: LuaTarget,
}

/// A multi-file project: per-file (relative path, source text) pairs plus
/// the extraction root for consumers that need the tree on disk (the
/// bundler resolves requires through the filesystem).
pub struct TestProject {
    pub name: &'static str,
    pub root: PathBuf,
    pub files: Vec<(String, String)>,
    pub version: LuaVersion,
    pub target: LuaTarget,
}

// Seeds and statement budget are fixed so numbers are comparable
// run to run. Each program ends in a top-level `return`, so wrap it in
// `do ... end` to keep the concatenation one valid chunk.
fn generated(version: LuaVersion) -> String {
    let mut source = String::new();
    for seed in 0..40 {
        source.push_str("do\n");
        source.push_str(&luck_testgen::generate_full(seed, version, 60));
        source.push_str("end\n");
    }
    source
}

// Pinned to a commit so bench inputs are immutable; the files are mirrored
// third-party code kept out of this repo.
const CORPUS_URL_BASE: &str = "https://raw.githubusercontent.com/esoware/luck-bench-corpus/9188b121d704c7fb2d0b2cd29e891bff0e57c384";

// Upstream project tarballs, pinned by commit SHA.
const ROACT_SHA: &str = "956891b70fdc5410e44e9664719cd0a9f7e6fabd"; // Roblox/roact v1.4.4
const PENLIGHT_SHA: &str = "e0bc8f7fce3b6a4fdef3660066f5006bf8456b32"; // lunarmodules/Penlight 1.15.0

fn cache_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

// CI runners see transient connection resets from GitHub's raw and
// codeload hosts; a failed fetch fails the whole bench job, so retry
// with backoff before giving up.
fn fetch_bytes(url: &str) -> Vec<u8> {
    let mut delay = std::time::Duration::from_secs(1);
    let mut last_error = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(delay);
            delay *= 2;
        }
        match ureq::get(url).call() {
            Ok(mut response) => match response.body_mut().read_to_vec() {
                Ok(bytes) => return bytes,
                Err(error) => last_error = error.to_string(),
            },
            Err(error) => last_error = error.to_string(),
        }
    }
    panic!("failed to fetch {url} after 3 attempts: {last_error}")
}

fn fetch_corpus_file(file_name: &str) -> String {
    let cache_path = cache_dir().join(file_name);
    if let Ok(text) = std::fs::read_to_string(&cache_path) {
        return text;
    }
    let url = format!("{CORPUS_URL_BASE}/{file_name}");
    let bytes = fetch_bytes(&url);
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("failed to read {url} as UTF-8: {error}"));
    let _ = std::fs::create_dir_all(cache_dir());
    let _ = std::fs::write(&cache_path, &text);
    text
}

/// Download and extract a pinned GitHub tarball into the corpus cache,
/// returning the extracted top-level directory (`<repo>-<sha>`).
fn fetch_project_tarball(owner_repo: &str, top_level: &str, sha: &str) -> PathBuf {
    let extracted = cache_dir().join(top_level);
    if extracted.is_dir() {
        return extracted;
    }
    let url = format!("https://codeload.github.com/{owner_repo}/tar.gz/{sha}");
    let bytes = fetch_bytes(&url);
    let _ = std::fs::create_dir_all(cache_dir());
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    archive
        .unpack(cache_dir())
        .unwrap_or_else(|error| panic!("failed to extract {url}: {error}"));
    extracted
}

/// All `.lua` files under `root` whose cache-relative path contains
/// `filter`, sorted by path for deterministic ordering.
fn collect_lua_files(root: &Path, filter: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("failed to list {}: {error}", dir.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "lua") {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("path under root")
                .to_string_lossy()
                .replace('\\', "/");
            if !relative.contains(filter) {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            files.push((relative, text));
        }
    }
    files.sort_by(|(a, _), (b, _)| a.cmp(b));
    files
}

// `module_pattern.lua` ends in a top-level `return`, so each fixture gets
// its own `do ... end` to keep the concatenation one valid chunk.
fn idiomatic() -> String {
    [
        include_str!("../../../tests/fixtures/idiomatic/control_flow.lua"),
        include_str!("../../../tests/fixtures/idiomatic/module_pattern.lua"),
        include_str!("../../../tests/fixtures/idiomatic/oop_self.lua"),
    ]
    .map(|fixture| format!("do\n{fixture}\nend\n"))
    .join("\n")
}

#[must_use]
pub fn test_files() -> Vec<TestFile> {
    vec![
        TestFile {
            file_name: "gen_full_lua54.lua",
            source_text: generated(LuaVersion::Lua54),
            version: LuaVersion::Lua54,
            target: LuaTarget::Lua54,
        },
        TestFile {
            file_name: "gen_full_luau.luau",
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

/// Deterministic multi-module require graph for the bundler bench. Real
/// projects don't bundle cleanly (Penlight requires stdlib modules and
/// builds require strings dynamically; Roact uses Roblox instance paths),
/// so the full-grammar generator supplies the module bodies and a diamond
/// DAG supplies the graph. Written fresh on every call so the tree always
/// matches the current generator.
#[must_use]
pub fn bundle_project_root() -> PathBuf {
    const MODULE_COUNT: usize = 40;
    let root = cache_dir().join("gen_bundle");
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", lib.display()));
    for index in 0..MODULE_COUNT {
        let mut source = String::new();
        for offset in [1, 3] {
            let dep = index + offset;
            if dep < MODULE_COUNT {
                source.push_str(&format!("local dep{dep} = require(\"lib.mod{dep}\")\n"));
            }
        }
        source.push_str(&luck_testgen::generate_full(
            index as u64,
            LuaVersion::Lua54,
            15,
        ));
        let path = lib.join(format!("mod{index}.lua"));
        std::fs::write(&path, source)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
    }
    let entry_path = root.join("main.lua");
    std::fs::write(
        &entry_path,
        "local root = require(\"lib.mod0\")\nreturn root\n",
    )
    .unwrap_or_else(|error| panic!("failed to write {}: {error}", entry_path.display()));
    root
}

/// The Penlight extraction root, for consumers that resolve files on disk.
#[must_use]
pub fn penlight_root() -> PathBuf {
    fetch_project_tarball(
        "lunarmodules/Penlight",
        &format!("Penlight-{PENLIGHT_SHA}"),
        PENLIGHT_SHA,
    )
}

#[must_use]
pub fn test_projects() -> Vec<TestProject> {
    let roact_root =
        fetch_project_tarball("Roblox/roact", &format!("roact-{ROACT_SHA}"), ROACT_SHA);
    let penlight_root = penlight_root();
    vec![
        TestProject {
            name: "roact",
            files: collect_lua_files(&roact_root.join("src"), ""),
            root: roact_root,
            version: LuaVersion::Luau,
            target: LuaTarget::LuauRoblox,
        },
        TestProject {
            name: "penlight",
            files: collect_lua_files(&penlight_root.join("lua"), "pl/"),
            root: penlight_root,
            version: LuaVersion::Lua51,
            target: LuaTarget::Lua51,
        },
    ]
}
