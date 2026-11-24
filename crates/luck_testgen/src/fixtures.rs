use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use luck_token::LuaVersion;

/// A Lua source in the shared fixture tree.
#[derive(Debug)]
pub struct LuaFixture {
    pub path: PathBuf,
    /// The path under the tree's root, which names the fixture.
    pub relative_path: PathBuf,
    /// The dialect the path names: `.luau` files and the `luau/` tree are
    /// Luau, `lua51/` to `lua55/` name their version, and everything else,
    /// `idiomatic/` included, is 5.4.
    pub version: LuaVersion,
    /// Under an `errors/` directory, so the fixture holds a mistake and may
    /// not parse.
    pub is_error: bool,
    pub source: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Every file in the shared fixture tree, in path order, whether it holds Lua
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

/// Every `.lua` and `.luau` file in the shared fixture tree, in path order.
///
/// # Panics
///
/// Panics as [`fixture_files`] does, or when a file isn't readable as UTF-8.
pub fn lua_fixtures() -> Vec<LuaFixture> {
    let root = root();
    fixture_files()
        .into_iter()
        .filter(|path| matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau")))
        .map(|path| {
            let relative_path =
                path.strip_prefix(&root).expect("a walked path is under the root").to_path_buf();
            let source = fs::read_to_string(&path).expect("a fixture is readable");
            let is_error =
                relative_path.components().any(|component| component.as_os_str() == "errors");
            LuaFixture {
                version: version_named_by(&relative_path),
                is_error,
                path,
                relative_path,
                source,
            }
        })
        .collect()
}

fn version_named_by(relative_path: &Path) -> LuaVersion {
    if relative_path.extension() == Some(OsStr::new("luau")) {
        return LuaVersion::Luau;
    }
    match relative_path.iter().next().and_then(OsStr::to_str) {
        Some("luau") => LuaVersion::Luau,
        Some("lua51") => LuaVersion::Lua51,
        Some("lua52") => LuaVersion::Lua52,
        Some("lua53") => LuaVersion::Lua53,
        Some("lua55") => LuaVersion::Lua55,
        _ => LuaVersion::Lua54,
    }
}
