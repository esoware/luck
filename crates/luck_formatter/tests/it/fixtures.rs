//! The shared fixtures at `tests/fixtures/`, run through verified formatting.
//!
//! Broad coverage for the invariants the hand-written cases pin one at a
//! time: AST equivalence, comment text and placement, and idempotency, over
//! real files in every dialect.

use std::path::{Path, PathBuf};

use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

#[test]
fn every_fixture_formats_and_verifies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let sources = collect_sources(&root);
    assert!(!sources.is_empty(), "no fixtures under {}", root.display());

    for (path, version) in sources {
        let source = std::fs::read_to_string(&path).expect("fixture is readable");
        for line_width in [1, 40, 80, 120] {
            let options = FormatOptions {
                line_width,
                ..FormatOptions::default()
            };
            let result = luck_formatter::format_and_verify(&source, version, &options)
                .unwrap_or_else(|(_, diff)| {
                    panic!("{} at width {line_width}: {diff:?}", path.display())
                });
            assert!(
                result.errors.is_empty(),
                "{} does not parse as {version:?}: {:?}",
                path.display(),
                result.errors
            );
        }
    }
}

/// Every fixture that is meant to parse, with the dialect its directory
/// declares. The `errors/` trees hold deliberately invalid input.
fn collect_sources(root: &Path) -> Vec<(PathBuf, LuaVersion)> {
    let mut sources = Vec::new();
    let mut directories = vec![(root.to_path_buf(), None)];
    while let Some((directory, version)) = directories.pop() {
        for entry in std::fs::read_dir(&directory).expect("fixture directory is readable") {
            let path = entry.expect("fixture entry is readable").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if name != "errors" {
                    let nested = version.or_else(|| dialect_of(name));
                    directories.push((path, nested));
                }
            } else if let Some(file_version) = source_dialect(&path, version) {
                sources.push((path, file_version));
            }
        }
    }
    sources
}

/// The dialect a top-level fixture directory names. `idiomatic/` is dialect
/// agnostic and holds plain Lua.
fn dialect_of(directory: &str) -> Option<LuaVersion> {
    match directory {
        "lua51" => Some(LuaVersion::Lua51),
        "lua52" => Some(LuaVersion::Lua52),
        "lua53" => Some(LuaVersion::Lua53),
        "lua54" | "idiomatic" => Some(LuaVersion::Lua54),
        "lua55" => Some(LuaVersion::Lua55),
        "luau" => Some(LuaVersion::Luau),
        _ => None,
    }
}

/// The dialect a source file formats as: `.luau` names its own, `.lua` takes
/// the one its directory declares. Anything else is not a source file.
fn source_dialect(path: &Path, directory: Option<LuaVersion>) -> Option<LuaVersion> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("luau") => Some(LuaVersion::Luau),
        Some("lua") => directory,
        _ => None,
    }
}
