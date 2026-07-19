//! # luck_resolver
//!
//! Module resolution for `require()` calls across Lua versions and Luau.
//!
//! **Lua 5.x** - Template-based search paths. Expands `require("foo.bar")` by substituting
//! into path templates like `./?.lua`, checking each candidate on disk.
//!
//! **Luau** - Relative imports (`./module`, `../module`) resolved from the requiring file's
//! directory. Supports `@alias` prefixes from project configuration.
//!
//! # Usage
//!
//! ```no_run
//! use luck_core::LuaTarget;
//! use std::path::Path;
//!
//! let paths = ["./?.lua".to_string()];
//! let resolved = luck_resolver::resolve("foo", LuaTarget::Lua54, "main.lua", &paths, Path::new("."), 0..3).unwrap();
//! assert!(resolved.path.ends_with(".lua"));
//! ```

#![allow(clippy::result_large_err)]

use luck_core::diagnostics::Diagnostic;
use luck_core::types::LuaTarget;
use std::path::Path;

mod luau;

pub use luau::clear_luaurc_cache;

/// Resolved file path for a `require()` string.
#[derive(Debug)]
pub struct ResolveResult {
    pub path: String,
    pub warnings: Vec<Diagnostic>,
}

pub fn resolve(
    require_string: &str,
    target: LuaTarget,
    current_file: &str,
    search_paths: &[String],
    rc_dir: &Path,
    require_span: std::ops::Range<usize>,
) -> Result<ResolveResult, Diagnostic> {
    if target.is_luau() {
        luau::resolve_luau(require_string, current_file, require_span)
    } else {
        resolve_lua(
            require_string,
            current_file,
            search_paths,
            rc_dir,
            require_span,
        )
    }
}

fn resolve_lua(
    require_string: &str,
    current_file: &str,
    search_paths: &[String],
    rc_dir: &Path,
    require_span: std::ops::Range<usize>,
) -> Result<ResolveResult, Diagnostic> {
    let transformed = require_string.replace('.', "/");
    let mut tried_paths = Vec::new();

    for template in search_paths {
        let relative_path = template.replace('?', &transformed);
        let abs_path = rc_dir.join(&relative_path);

        if abs_path.is_file() {
            let normalized = normalize_path(&abs_path);
            return Ok(ResolveResult {
                path: normalized,
                warnings: Vec::new(),
            });
        }

        tried_paths.push(relative_path);
    }

    Err(luck_core::diagnostics::errors::e004(
        current_file,
        require_span,
        require_string,
        &tried_paths,
    ))
}

pub fn normalize_path(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let normalized = path.to_string_lossy().replace('\\', "/");
    // Windows extended-length prefixes after backslash replacement:
    //   \\?\C:\x        -> //?/C:/x        -> C:/x
    //   \\?\UNC\srv\sh  -> //?/UNC/srv/sh  -> //srv/sh (network path!)
    // Blindly stripping `//?/` turned network paths into RELATIVE
    // garbage (`UNC/srv/share/...`).
    if let Some(unc) = normalized.strip_prefix("//?/UNC/") {
        return format!("//{unc}");
    }
    normalized
        .strip_prefix("//?/")
        .unwrap_or(&normalized)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_lua_project() -> TempDir {
        let dir = TempDir::new().expect("tempdir creation failed");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("failed to create dir");
        fs::write(src.join("utils.lua"), "return {}").expect("failed to write file");
        fs::write(src.join("main.lua"), "local u = require('utils')")
            .expect("failed to write file");

        let lib = dir.path().join("lib");
        fs::create_dir_all(&lib).expect("failed to create dir");
        fs::write(lib.join("helper.lua"), "return {}").expect("failed to write file");

        let comp = src.join("components");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.lua"), "return {}").expect("failed to write file");

        dir
    }

    #[test]
    fn test_lua_resolve_basic() {
        let dir = setup_lua_project();
        let search_paths = vec!["src/?.lua".to_string(), "src/?/init.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let result = resolve_lua("utils", &current_file, &search_paths, dir.path(), 0..10)
            .expect("resolve failed");
        assert!(result.path.ends_with("src/utils.lua"));
    }

    #[test]
    fn test_lua_resolve_with_dots() {
        let dir = setup_lua_project();
        let nested = dir.path().join("src/foo");
        fs::create_dir_all(&nested).expect("failed to create dir");
        fs::write(nested.join("bar.lua"), "return {}").expect("failed to write file");

        let search_paths = vec!["src/?.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let result = resolve_lua("foo.bar", &current_file, &search_paths, dir.path(), 0..10)
            .expect("resolve failed");
        assert!(result.path.ends_with("src/foo/bar.lua"));
    }

    #[test]
    fn test_lua_resolve_init_module() {
        let dir = setup_lua_project();
        let search_paths = vec!["src/?.lua".to_string(), "src/?/init.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let result = resolve_lua(
            "components",
            &current_file,
            &search_paths,
            dir.path(),
            0..10,
        )
        .expect("resolve failed");
        assert!(result.path.ends_with("src/components/init.lua"));
    }

    #[test]
    fn test_lua_resolve_lib_fallback() {
        let dir = setup_lua_project();
        let search_paths = vec!["src/?.lua".to_string(), "lib/?.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let result = resolve_lua("helper", &current_file, &search_paths, dir.path(), 0..10)
            .expect("resolve failed");
        assert!(result.path.ends_with("lib/helper.lua"));
    }

    #[test]
    fn test_lua_resolve_not_found() {
        let dir = setup_lua_project();
        let search_paths = vec!["src/?.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let err = resolve_lua(
            "nonexistent",
            &current_file,
            &search_paths,
            dir.path(),
            0..10,
        )
        .unwrap_err();
        assert_eq!(err.code, "E004");
        assert!(
            err.help
                .expect("resolve failed")
                .contains("src/nonexistent.lua")
        );
    }
}
