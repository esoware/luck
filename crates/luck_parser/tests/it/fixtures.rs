use core::fmt::Write as _;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use luck_parser::parse;
use luck_token::LuaVersion;

/// The dialect of a fixture at `relative` under the fixtures root: `.luau`
/// files and the `luau/` tree are Luau, `lua51/` to `lua55/` name their
/// version, and everything else, `idiomatic/` included, is 5.4.
fn detect_version(relative: &Path) -> LuaVersion {
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

fn is_error_fixture(relative: &Path) -> bool {
    relative.components().any(|component| component.as_os_str() == "errors")
}

fn fixtures_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has parent")
        .parent()
        .expect("crates dir has parent")
        .join("tests")
        .join("fixtures")
}

/// Every `.lua` and `.luau` file under `directory`, in path order.
fn collect_fixture_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk_directory(directory, &mut files);
    files.sort();
    files
}

fn walk_directory(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_directory(&path, files);
        } else {
            let extension = path.extension().and_then(OsStr::to_str);
            if matches!(extension, Some("lua" | "luau")) {
                files.push(path);
            }
        }
    }
}

#[test]
fn parse_all_fixtures() {
    let root = fixtures_directory();
    let files = collect_fixture_files(&root);
    assert!(!files.is_empty(), "no fixture files found");

    let mut failures = Vec::new();
    let mut success_count = 0usize;

    for path in &files {
        let relative = path.strip_prefix(&root).expect("fixture is under the root");
        let source = fs::read_to_string(path).expect("failed to read fixture file");
        let result = parse(&source, detect_version(relative));

        if is_error_fixture(relative) {
            // Fixtures under `errors/` may or may not parse.
            continue;
        }

        if result.errors.is_empty() {
            success_count = success_count.saturating_add(1);
        } else {
            failures.push((path.clone(), result.errors));
        }
    }

    if !failures.is_empty() {
        let mut report = format!("{} fixture(s) failed to parse:\n", failures.len());
        for (path, errors) in &failures {
            writeln!(report, "  {}:", path.display()).unwrap();
            for error in errors {
                writeln!(report, "    - {}", error.message).unwrap();
            }
        }
        panic!("{report}{success_count} fixture(s) parsed successfully");
    }

    assert!(success_count > 0, "parsed {success_count} fixtures successfully");
}
