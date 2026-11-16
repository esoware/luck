//! Pinned bench inputs, two kinds:
//!
//! - five single files and the bundler's module tree in
//!   esoware/luck-bench-corpus, fetched as one tarball;
//! - two real multi-file projects fetched as pinned upstream tarballs:
//!   Roact (Roblox Luau, ~80 files) and Penlight (Lua 5.1, ~40 files).
//!
//! Everything lands in the gitignored `corpus/` cache and is pinned to a
//! commit SHA, so an input changes only when its pin does.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a bench has no way to report a failed corpus load"
)]

use std::fs;
use std::path::{Path, PathBuf};

use luck_core::LuaTarget;
use luck_parser::ParseResult;
use luck_token::LuaVersion;

use crate::corpus_cache::fetch_project_tarball;

#[derive(Debug)]
pub struct TestFile {
    pub file_name: &'static str,
    pub source_text: String,
    pub target: LuaTarget,
}

impl TestFile {
    /// # Panics
    ///
    /// Panics when the file doesn't parse cleanly.
    #[must_use]
    pub fn parse(&self) -> ParseResult {
        parse_cleanly(self.file_name, &self.source_text, self.target.lua_version())
    }
}

#[derive(Debug)]
pub struct ProjectFile {
    pub relative_path: String,
    pub source_text: String,
}

#[derive(Debug)]
pub struct TestProject {
    pub name: &'static str,
    pub files: Vec<ProjectFile>,
    pub target: LuaTarget,
}

impl TestProject {
    /// # Panics
    ///
    /// Panics when a file doesn't parse cleanly.
    #[must_use]
    pub fn parse_files(&self) -> Vec<ParseResult> {
        let version = self.target.lua_version();
        self.files
            .iter()
            .map(|file| parse_cleanly(&file.relative_path, &file.source_text, version))
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

const CORPUS_SHA: &str = "1922f00013366c3024f1fb95f62a6aa14c63a95a"; // esoware/luck-bench-corpus
const ROACT_SHA: &str = "956891b70fdc5410e44e9664719cd0a9f7e6fabd"; // Roblox/roact v1.4.4
const PENLIGHT_SHA: &str = "e0bc8f7fce3b6a4fdef3660066f5006bf8456b32"; // lunarmodules/Penlight 1.15.0

fn corpus_root() -> PathBuf {
    fetch_project_tarball("esoware", "luck-bench-corpus", CORPUS_SHA)
}

/// # Panics
///
/// Panics when the file can't be read.
fn corpus_file(root: &Path, file_name: &'static str, target: LuaTarget) -> TestFile {
    let path = root.join(file_name);
    let source_text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    TestFile { file_name, source_text, target }
}

/// # Panics
///
/// Panics when the tree can't be read.
fn collect_lua_files(root: &Path, filter: &str) -> Vec<ProjectFile> {
    let mut files = Vec::new();
    let mut pending_directories = vec![root.to_path_buf()];
    while let Some(directory) = pending_directories.pop() {
        let entries = fs::read_dir(&directory)
            .unwrap_or_else(|e| panic!("failed to list {}: {e}", directory.display()));
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
            let source_text = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            files.push(ProjectFile { relative_path, source_text });
        }
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    files
}

#[must_use]
pub fn test_files() -> Vec<TestFile> {
    let root = corpus_root();
    vec![
        // 40 full-grammar `luck_testgen` programs, each in its own
        // `do ... end`: every version-gated construct in a realistic mix.
        corpus_file(&root, "gen_full_lua54.lua", LuaTarget::Lua54),
        corpus_file(&root, "gen_full_luau.luau", LuaTarget::Luau),
        // The `tests/fixtures/idiomatic` files, a small hand-written sample.
        corpus_file(&root, "idiomatic.lua", LuaTarget::Lua54),
        // Real-world Roblox Luau: 13k lines of comment- and string-heavy
        // hand-written code (Infinite Yield admin script).
        corpus_file(&root, "infinite_yield.luau", LuaTarget::LuauRoblox),
        // Adversarial: 2.2 MB of VM-obfuscated Luau on a single line.
        corpus_file(&root, "obfuscated_vm.luau", LuaTarget::LuauRoblox),
    ]
}

/// Real projects don't bundle cleanly: Penlight requires stdlib modules and
/// builds require strings dynamically, and Roact uses Roblox instance paths.
///
/// So this tree is a 40-module diamond DAG of generated Lua 5.4, entered at
/// `main.lua`.
#[must_use]
pub fn bundle_project_root() -> PathBuf {
    corpus_root().join("gen_bundle")
}

#[must_use]
pub fn test_projects() -> Vec<TestProject> {
    let roact_root = fetch_project_tarball("Roblox", "roact", ROACT_SHA);
    let penlight_root = fetch_project_tarball("lunarmodules", "Penlight", PENLIGHT_SHA);
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
