use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::{fs, io};

use luck_token::LuaVersion;

/// The dialect of a fixture at `relative` under the fixtures root: `.luau`
/// files and the `luau/` tree are Luau, `lua51/` to `lua55/` name their
/// version, and everything else, `idiomatic/` included, is 5.4.
#[must_use]
pub fn detect_version(relative: &Path) -> LuaVersion {
    if relative.extension() == Some(OsStr::new("luau")) {
        return LuaVersion::Luau;
    }
    match relative.iter().next().and_then(OsStr::to_str) {
        Some("luau") => LuaVersion::Luau,
        Some("lua51") => LuaVersion::Lua51,
        Some("lua52") => LuaVersion::Lua52,
        Some("lua53") => LuaVersion::Lua53,
        Some("lua55") => LuaVersion::Lua55,
        _ => LuaVersion::Lua54,
    }
}

#[must_use]
pub fn is_error_fixture(relative: &Path) -> bool {
    relative.components().any(|component| component.as_os_str() == "errors")
}

/// Every file under `root`, in path order. Callers filter by extension, since
/// not every harness reads only Lua sources.
///
/// # Errors
///
/// Fails when a directory or one of its entries can't be read.
pub fn collect_fixture_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                directories.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}
