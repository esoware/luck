//! Pinned inputs, two kinds:
//!
//! - three single files and the programs of the bundler's module tree in
//!   esoware/luck-benchmark-files, fetched as one tarball;
//! - two real multi-file projects fetched as pinned upstream tarballs:
//!   Roact (Roblox Luau, ~80 files) and Penlight (portable code that reads as
//!   Luau, ~40 files).
//!
//! Everything lands in the gitignored `cache/` directory and is pinned to a
//! commit SHA, so an input changes only when its pin does.

use std::fs;
use std::io::{self, Write as _};
use std::ops::Range;
use std::path::{Path, PathBuf};

use luck_allocator::Allocator;
use luck_ast::GetSpan as _;
use luck_parser::ParseResult;
use luck_token::Variant;

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

const BENCHMARK_FILES_SHA: &str = "c8c611aef209cad7a33495b2e36135e99e8b84f1"; // esoware/luck-benchmark-files
const ROACT_SHA: &str = "956891b70fdc5410e44e9664719cd0a9f7e6fabd"; // Roblox/roact v1.4.4
const PENLIGHT_SHA: &str = "e0bc8f7fce3b6a4fdef3660066f5006bf8456b32"; // lunarmodules/Penlight 1.15.0

/// How many modules [`bundle_project_root`] splits the programs into.
const BUNDLE_MODULE_COUNT: usize = 40;

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
            if path.extension().is_none_or(|extension| extension != "lua") {
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
        benchmark_file(&root, "gen_full_luau.luau", Variant::Standalone),
        // The `tests/fixtures/idiomatic` files, a small hand-written sample.
        benchmark_file(&root, "idiomatic.lua", Variant::Standalone),
        // Real-world Roblox Luau: 13k lines of comment- and string-heavy
        // hand-written code (Infinite Yield admin script).
        benchmark_file(&root, "infinite_yield.luau", Variant::Roblox),
    ]
}

/// Real projects don't bundle cleanly: Penlight requires its modules by
/// dotted name, which Luau doesn't resolve, and Roact requires Roblox
/// instances.
///
/// So this tree is the programs of `gen_full_luau.luau` split into a 40-module
/// diamond DAG, in which each module requires the next one and the third one
/// on, entered at `main.luau`. The pinned tarball's own `gen_bundle` tree
/// doesn't parse as Luau, so this one is written beside it the first time
/// it's asked for.
///
/// # Panics
///
/// Panics when the tree can't be written.
pub fn bundle_project_root() -> PathBuf {
    let root = benchmark_files_root();
    let project = root.join("gen_bundle_luau");
    if !project.is_dir() {
        write_bundle_project(&root.join("gen_full_luau.luau"), &project)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", project.display()));
    }
    project
}

/// Writes the tree aside and renames it into place, so `project` is absent
/// until it's whole.
fn write_bundle_project(programs_path: &Path, project: &Path) -> io::Result<()> {
    let source = fs::read(programs_path)?;
    let allocator = Allocator::new();
    let parsed = parse_cleanly(&allocator, "gen_full_luau.luau", &source);
    let statements = &parsed.chunk.block.stmts;
    let groups: Vec<_> =
        statements.chunks(statements.len().div_ceil(BUNDLE_MODULE_COUNT).max(1)).collect();
    let partial = project.with_extension("partial");
    let _ = fs::remove_dir_all(&partial);
    fs::create_dir_all(partial.join("lib"))?;
    fs::write(partial.join("main.luau"), "return require(\"./lib/mod0\")\n")?;
    for (index, group) in groups.iter().enumerate() {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let dependencies: Vec<usize> =
            [index + 1, index + 3].into_iter().filter(|&other| other < groups.len()).collect();
        let mut module = Vec::new();
        for dependency in &dependencies {
            writeln!(module, "local mod{dependency} = require(\"./mod{dependency}\")")?;
        }
        module.extend_from_slice(b"do\n");
        module.extend_from_slice(&source[Range::<usize>::from(first.span().merge(last.span()))]);
        module.extend_from_slice(b"\nend\n");
        let returned: Vec<String> =
            dependencies.iter().map(|dependency| format!("mod{dependency}")).collect();
        writeln!(module, "return {{ {} }}", returned.join(", "))?;
        fs::write(partial.join("lib").join(format!("mod{index}.luau")), module)?;
    }
    fs::rename(&partial, project)
}

pub fn test_projects() -> Vec<TestProject> {
    let roact_root = fetch_project_tarball("Roblox", "roact", ROACT_SHA);
    let penlight_root = fetch_project_tarball("lunarmodules", "Penlight", PENLIGHT_SHA);
    vec![
        TestProject {
            name: "roact",
            files: collect_source_files(&roact_root.join("src")),
            variant: Variant::Roblox,
        },
        TestProject {
            name: "penlight",
            files: collect_source_files(&penlight_root.join("lua")),
            variant: Variant::Standalone,
        },
    ]
}
