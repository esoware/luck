use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// A source in the shared fixture tree.
#[derive(Debug)]
pub struct SourceFixture {
    pub path: PathBuf,
    /// The path under the tree's root, which names the fixture.
    pub relative_path: PathBuf,
    /// Under an `errors/` directory, so the fixture holds a mistake and may
    /// not parse.
    pub is_error: bool,
    pub source: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Every file in the shared fixture tree, in path order, whether it holds Luau
/// or not.
///
/// # Panics
///
/// Panics when the tree can't be read, or holds no file.
pub fn fixture_files() -> Vec<PathBuf> {
    let root = root();
    let mut files = Vec::new();
    let mut directories = vec![root.clone()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).expect("the fixture tree is readable") {
            let path = entry.expect("the fixture tree is readable").path();
            if path.is_dir() {
                directories.push(path);
            } else {
                files.push(path);
            }
        }
    }
    assert!(!files.is_empty(), "no fixtures under {}", root.display());
    files.sort();
    files
}

/// Every `.luau` and `.lua` file in the shared fixture tree, in path order,
/// but the definition files [`definition_fixtures`] gives.
///
/// # Panics
///
/// Panics as [`fixture_files`] does, or when a file isn't readable as UTF-8.
pub fn source_fixtures() -> Vec<SourceFixture> {
    luau_fixtures().into_iter().filter(|fixture| !is_definition_file(fixture)).collect()
}

/// Every Luau file under the shared fixture tree's `definitions/`, in path
/// order, each a definition file, which parses only as one.
///
/// # Panics
///
/// Panics as [`source_fixtures`] does.
pub fn definition_fixtures() -> Vec<SourceFixture> {
    luau_fixtures().into_iter().filter(is_definition_file).collect()
}

fn is_definition_file(fixture: &SourceFixture) -> bool {
    fixture.relative_path.starts_with("definitions")
}

fn luau_fixtures() -> Vec<SourceFixture> {
    let root = root();
    fixture_files()
        .into_iter()
        .filter(|path| matches!(path.extension().and_then(OsStr::to_str), Some("luau" | "lua")))
        .map(|path| {
            let relative_path =
                path.strip_prefix(&root).expect("a walked path is under the root").to_path_buf();
            let source = fs::read_to_string(&path).expect("a fixture is readable");
            let is_error =
                relative_path.components().any(|component| component.as_os_str() == "errors");
            SourceFixture { path, relative_path, is_error, source }
        })
        .collect()
}
