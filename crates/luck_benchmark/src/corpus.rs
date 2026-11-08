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

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a bench has no way to report a failed corpus load"
)]

use core::fmt::Write as _;
use core::time::Duration;
use std::path::{Path, PathBuf};
use std::{fs, thread};

use flate2::read::GzDecoder;
use luck_core::LuaTarget;
use luck_parser::ParseResult;
use luck_token::LuaVersion;
use tar::Archive;

#[derive(Debug)]
pub struct TestFile {
    pub file_name: &'static str,
    pub source_text: String,
    pub target: LuaTarget,
}

impl TestFile {
    /// Parses the file, for benches that time a later stage.
    ///
    /// # Panics
    ///
    /// Panics when the file doesn't parse cleanly.
    #[must_use]
    pub fn parse(&self) -> ParseResult {
        parse_cleanly(self.file_name, &self.source_text, self.target.lua_version())
    }
}

/// A multi-file project as (relative path, source text) pairs.
#[derive(Debug)]
pub struct TestProject {
    pub name: &'static str,
    pub files: Vec<(String, String)>,
    pub target: LuaTarget,
}

impl TestProject {
    /// Parses every file, for benches that time a later stage.
    ///
    /// # Panics
    ///
    /// Panics when a file doesn't parse cleanly.
    #[must_use]
    pub fn parse_files(&self) -> Vec<ParseResult> {
        let version = self.target.lua_version();
        self.files
            .iter()
            .map(|(path, source_text)| parse_cleanly(path, source_text, version))
            .collect()
    }
}

/// # Panics
///
/// Panics when `source_text` doesn't parse cleanly.
fn parse_cleanly(name: &str, source_text: &str, version: LuaVersion) -> ParseResult {
    let parse_result = luck_parser::parse(source_text, version);
    assert!(parse_result.errors.is_empty(), "{name}: {:?}", parse_result.errors);
    parse_result
}

// Each program ends in a top-level `return`, so it gets a `do ... end`
// wrapper to keep the concatenation one valid chunk. Luau value exports are
// only valid at module scope, hence the embeddable variant.
fn generated(version: LuaVersion) -> String {
    let mut source = String::new();
    for seed in 0..40 {
        source.push_str("do\n");
        source.push_str(&luck_testgen::generate_full_embeddable(seed, version, 60));
        source.push_str("end\n");
    }
    source
}

// Third-party files kept out of this repo, pinned to a commit.
const CORPUS_URL_BASE: &str = "https://raw.githubusercontent.com/esoware/luck-bench-corpus/9188b121d704c7fb2d0b2cd29e891bff0e57c384";

const ROACT_SHA: &str = "956891b70fdc5410e44e9664719cd0a9f7e6fabd"; // Roblox/roact v1.4.4
const PENLIGHT_SHA: &str = "e0bc8f7fce3b6a4fdef3660066f5006bf8456b32"; // lunarmodules/Penlight 1.15.0

fn cache_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

/// Fetches `url`, retrying with backoff.
///
/// CI runners see transient connection resets from GitHub's raw and
/// codeload hosts, and one failed fetch would fail the whole bench job.
///
/// # Panics
///
/// Panics when every attempt fails.
fn fetch_bytes(url: &str) -> Vec<u8> {
    let mut last_error = String::new();
    for delay in [Duration::ZERO, Duration::from_secs(1), Duration::from_secs(2)] {
        thread::sleep(delay);
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

/// Reads `file_name` from the corpus cache, fetching it on a miss.
///
/// # Panics
///
/// Panics when the file can't be fetched or isn't UTF-8.
fn fetch_corpus_file(file_name: &str) -> String {
    let cache_path = cache_directory().join(file_name);
    if let Ok(text) = fs::read_to_string(&cache_path) {
        return text;
    }
    let url = format!("{CORPUS_URL_BASE}/{file_name}");
    let bytes = fetch_bytes(&url);
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("failed to read {url} as UTF-8: {error}"));
    let _unused =
        fs::create_dir_all(cache_directory()).and_then(|()| fs::write(&cache_path, &text));
    text
}

/// Downloads and extracts a pinned GitHub tarball into the corpus cache
/// once, and returns its top-level `<repo>-<sha>` directory.
///
/// # Panics
///
/// Panics when the tarball can't be fetched or extracted.
fn fetch_project_tarball(repository: &str, top_level: &str, sha: &str) -> PathBuf {
    let extracted = cache_directory().join(top_level);
    if extracted.is_dir() {
        return extracted;
    }
    let url = format!("https://codeload.github.com/{repository}/tar.gz/{sha}");
    let bytes = fetch_bytes(&url);
    let _unused = fs::create_dir_all(cache_directory());
    let mut archive = Archive::new(GzDecoder::new(bytes.as_slice()));
    archive
        .unpack(cache_directory())
        .unwrap_or_else(|error| panic!("failed to extract {url}: {error}"));
    extracted
}

/// All `.lua` files under `root` whose path relative to `root` contains
/// `filter`, sorted by path for deterministic ordering.
///
/// # Panics
///
/// Panics when the tree can't be read.
fn collect_lua_files(root: &Path, filter: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut pending_directories = vec![root.to_path_buf()];
    while let Some(directory) = pending_directories.pop() {
        let entries = fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("failed to list {}: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending_directories.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "lua") {
                continue;
            }
            let relative_path =
                path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            if !relative_path.contains(filter) {
                continue;
            }
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            files.push((relative_path, text));
        }
    }
    files.sort_by(|(left, _), (right, _)| left.cmp(right));
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
            target: LuaTarget::Lua54,
        },
        TestFile {
            file_name: "gen_full_luau.luau",
            source_text: generated(LuaVersion::Luau),
            target: LuaTarget::Luau,
        },
        TestFile { file_name: "idiomatic.lua", source_text: idiomatic(), target: LuaTarget::Lua54 },
        // Real-world Roblox Luau: 13k lines of comment- and string-heavy
        // hand-written code (Infinite Yield admin script).
        TestFile {
            file_name: "infinite_yield.luau",
            source_text: fetch_corpus_file("infinite_yield.luau"),
            target: LuaTarget::LuauRoblox,
        },
        // Adversarial: 2.2 MB of VM-obfuscated Luau on a single line.
        TestFile {
            file_name: "obfuscated_vm.luau",
            source_text: fetch_corpus_file("obfuscated_vm.luau"),
            target: LuaTarget::LuauRoblox,
        },
    ]
}

/// Deterministic multi-module require graph for the bundler bench.
///
/// Real projects don't bundle cleanly (Penlight requires stdlib modules and
/// builds require strings dynamically; Roact uses Roblox instance paths),
/// so the full-grammar generator supplies the module bodies and a diamond
/// DAG supplies the graph. Written fresh on every call so the tree always
/// matches the current generator.
///
/// # Panics
///
/// Panics when the tree can't be written to disk.
#[must_use]
pub fn bundle_project_root() -> PathBuf {
    const MODULE_COUNT: u64 = 40;
    let root = cache_directory().join("gen_bundle");
    let lib_directory = root.join("lib");
    fs::create_dir_all(&lib_directory)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", lib_directory.display()));
    for index in 0..MODULE_COUNT {
        let mut source = String::new();
        for offset in [1, 3] {
            let dependency = index.saturating_add(offset);
            if dependency < MODULE_COUNT {
                let _unused =
                    writeln!(source, "local dep{dependency} = require(\"lib.mod{dependency}\")");
            }
        }
        source.push_str(&luck_testgen::generate_full(index, LuaVersion::Lua54, 15));
        let path = lib_directory.join(format!("mod{index}.lua"));
        fs::write(&path, source)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
    }
    let entry_path = root.join("main.lua");
    fs::write(&entry_path, "local root = require(\"lib.mod0\")\nreturn root\n")
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", entry_path.display()));
    root
}

#[must_use]
pub fn test_projects() -> Vec<TestProject> {
    let roact_root =
        fetch_project_tarball("Roblox/roact", &format!("roact-{ROACT_SHA}"), ROACT_SHA);
    let penlight_root = fetch_project_tarball(
        "lunarmodules/Penlight",
        &format!("Penlight-{PENLIGHT_SHA}"),
        PENLIGHT_SHA,
    );
    vec![
        TestProject {
            name: "roact",
            files: collect_lua_files(&roact_root.join("src"), ""),
            target: LuaTarget::LuauRoblox,
        },
        TestProject {
            name: "penlight",
            files: collect_lua_files(&penlight_root.join("lua"), "pl/"),
            target: LuaTarget::Lua51,
        },
    ]
}
