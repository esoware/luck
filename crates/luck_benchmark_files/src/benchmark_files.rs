//! Pinned inputs, two kinds:
//!
//! - four single files, Penlight 1.15.0's modules (portable Lua 5.1 that
//!   reads as Luau, ~40 files), and a 40-module tree for the bundler in
//!   esoware/luck-benchmark-files, fetched as one tarball;
//! - two real multi-file projects fetched as pinned upstream tarballs:
//!   Roact (Roblox Luau, ~80 files) and rbx-cryptography (hashing,
//!   encryption, and signatures in `bit32` and `buffer` arithmetic, ~60 files
//!   and 26k lines), the second of which also bundles.
//!
//! Everything lands in the gitignored `cache/` directory and is pinned to a
//! commit SHA, so an input changes only when its pin does.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use luck_allocator::Allocator;
use luck_parser::ParseResult;
use luck_stdlib::Variant;

use crate::download::fetch_project_tarball;

#[derive(Debug)]
pub struct TestFile {
    pub file_name: &'static str,
    pub source_text: Vec<u8>,
    pub variant: Variant,
}

impl TestFile {
    /// # Panics
    ///
    /// Panics when the file doesn't parse cleanly.
    pub fn parse<'ast>(&'ast self, allocator: &'ast Allocator) -> ParseResult<'ast> {
        parse_cleanly(allocator, self.file_name, &self.source_text)
    }
}

#[derive(Debug)]
pub struct ProjectFile {
    pub relative_path: String,
    pub source_text: Vec<u8>,
}

#[derive(Debug)]
pub struct TestProject {
    pub name: &'static str,
    pub files: Vec<ProjectFile>,
    pub variant: Variant,
}

impl TestProject {
    /// # Panics
    ///
    /// Panics when a file doesn't parse cleanly.
    pub fn parse_files<'ast>(&'ast self, allocator: &'ast Allocator) -> Vec<ParseResult<'ast>> {
        self.files
            .iter()
            .map(|file| parse_cleanly(allocator, &file.relative_path, &file.source_text))
            .collect()
    }
}

/// `source_text` parsed with its tokens, which the formatter reads, so every
/// stage can take the one parse.
fn parse_cleanly<'ast>(
    allocator: &'ast Allocator,
    name: &str,
    source_text: &'ast [u8],
) -> ParseResult<'ast> {
    let parse_result = luck_parser::parse_with_tokens(allocator, source_text);
    assert!(parse_result.errors.is_empty(), "{name}: {:?}", parse_result.errors);
    parse_result
}

const BENCHMARK_FILES_SHA: &str = "8f7af261d1d60c76297c9f0c69224e173a0c4e8a"; // esoware/luck-benchmark-files
const ROACT_SHA: &str = "956891b70fdc5410e44e9664719cd0a9f7e6fabd"; // Roblox/roact v1.4.4
const CRYPTOGRAPHY_SHA: &str = "26226eeecbb7314b91095f3dea4d4b4444495247"; // daily3014/rbx-cryptography v4.1.2

fn cryptography_root() -> PathBuf {
    fetch_project_tarball("daily3014", "rbx-cryptography", CRYPTOGRAPHY_SHA)
}

fn benchmark_files_root() -> PathBuf {
    fetch_project_tarball("esoware", "luck-benchmark-files", BENCHMARK_FILES_SHA)
}

fn benchmark_file(root: &Path, file_name: &'static str, variant: Variant) -> TestFile {
    let path = root.join(file_name);
    let source_text =
        fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    TestFile { file_name, source_text, variant }
}

fn collect_source_files(root: &Path) -> Vec<ProjectFile> {
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
            if !matches!(path.extension().and_then(OsStr::to_str), Some("luau" | "lua")) {
                continue;
            }
            let relative_path = path
                .strip_prefix(root)
                .expect("a walked path is under the root")
                .to_string_lossy()
                .replace('\\', "/");
            let source_text = fs::read(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            files.push(ProjectFile { relative_path, source_text });
        }
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    files
}

pub fn test_files() -> Vec<TestFile> {
    let root = benchmark_files_root();
    vec![
        // 40 full-grammar `luck_testgen` programs, each in its own
        // `do ... end`: every construct in a realistic mix.
        benchmark_file(&root, "gen_full.luau", Variant::Standalone),
        // The `tests/fixtures/idiomatic` files, a small hand-written sample.
        benchmark_file(&root, "idiomatic.luau", Variant::Standalone),
        // Real-world Roblox Luau: 13k lines of comment- and string-heavy
        // hand-written code (Infinite Yield admin script).
        benchmark_file(&root, "infinite_yield.luau", Variant::Roblox),
        // 6k lines that spell every form of Luau's syntax, nested and
        // knotted, and pin the shapes that once broke a parser or compiler.
        // It parses but isn't meant to run.
        benchmark_file(&root, "stress.luau", Variant::Standalone),
    ]
}

/// A tree `luck_bundler` bundles from `entry`, with module paths relative to
/// `root`.
#[derive(Debug)]
pub struct BundleProject {
    pub name: &'static str,
    pub root: PathBuf,
    pub entry: PathBuf,
}

/// Roact requires Roblox instances, and Penlight its modules by dotted name,
/// which no bundle can resolve, so beside rbx-cryptography, whose requires
/// are all relative, the bundler measures the programs of `gen_full.luau`
/// split into a 40-module diamond DAG, in which each module requires the next
/// one and the third one on.
pub fn bundle_projects() -> Vec<BundleProject> {
    let generated = benchmark_files_root().join("gen_bundle");
    let cryptography = cryptography_root();
    vec![
        BundleProject { name: "gen_modules", entry: generated.join("main.luau"), root: generated },
        BundleProject {
            name: "rbx_cryptography",
            entry: cryptography.join("src").join("init.luau"),
            root: cryptography,
        },
    ]
}

pub fn test_projects() -> Vec<TestProject> {
    let roact_root = fetch_project_tarball("Roblox", "roact", ROACT_SHA);
    vec![
        TestProject {
            name: "roact",
            files: collect_source_files(&roact_root.join("src")),
            variant: Variant::Roblox,
        },
        TestProject {
            name: "rbx_cryptography",
            files: collect_source_files(&cryptography_root().join("src")),
            variant: Variant::Roblox,
        },
        TestProject {
            name: "penlight",
            files: collect_source_files(&benchmark_files_root().join("penlight")),
            variant: Variant::Standalone,
        },
    ]
}
