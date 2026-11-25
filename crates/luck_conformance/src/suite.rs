//! Luau's conformance tests, which Luau keeps in its repository, pinned to
//! the commit of the release tag of the interpreter mlua vendors.

use std::path::{Path, PathBuf};
use std::{fs, io};

use luck_benchmark_files::{extract_once, fetch_bytes};

/// The `_VERSION` of the interpreter the suite is pinned for.
const INTERPRETER: &str = "Luau 0.748";
/// luau-lang/luau at the 0.748 tag.
const URL: &str =
    "https://codeload.github.com/luau-lang/luau/tar.gz/3d91c9d5c280c95bfc4be63aac46012cf861d394";
/// Where the tests sit in the tarball.
const TARBALL_DIRECTORY: &str = "luau-3d91c9d5c280c95bfc4be63aac46012cf861d394/tests/conformance";
/// The cache directory the tests unpack into.
const DIRECTORY: &str = "luau-0.748";

/// The Luau flags for what luck reads ahead of the pinned release, exported
/// values and conditional bindings, and for the `buffer` vector functions
/// `buffers.luau` calls, which Luau's runner turns on for it.
pub(crate) const FAST_FLAGS: &[&str] = &[
    "LuauExportValueSyntax",
    "LuauExperimentalIfLocalSyntax",
    "LuauExperimentalWhileLocalSyntax",
    "DebugLuauBufferVectorLibrary",
];

/// A test file of the suite, as bytes, which luck parses.
#[derive(Debug)]
pub(crate) struct SuiteFile {
    /// The file's name in the suite's directory, such as `strings.luau`, as
    /// the snapshots name it.
    pub name: String,
    pub source: Vec<u8>,
}

pub(crate) fn check_interpreter(interpreter: &str) -> io::Result<()> {
    if interpreter == INTERPRETER {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the suite is pinned for `{INTERPRETER}`, but mlua links `{interpreter}`; pin the release's tests in suite.rs"
        )))
    }
}

/// The suite's directory in the cache, downloaded into it on first use.
pub(crate) fn fetch() -> io::Result<PathBuf> {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("cache");
    extract_once(&cache, DIRECTORY, URL, TARBALL_DIRECTORY, fetch_bytes)
}

/// The suite's test files in `root`, or only those whose name contains
/// `filter`, in name order, which every snapshot lists them in.
pub(crate) fn files(root: &Path, filter: Option<&str>) -> io::Result<Vec<SuiteFile>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "luau") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if filter.is_some_and(|part| !name.contains(part)) {
            continue;
        }
        files.push(SuiteFile { name: name.to_owned(), source: fs::read(&path)? });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}
