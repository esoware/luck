//! # luck_resolver
//!
//! Maps the string argument of a `require()` call to a filesystem path.
//!
//! **Lua 5.x** - template-based search paths. `require("foo.bar")` substitutes
//! `foo/bar` into each template (`./?.lua`, `./?/init.lua`, ...) and returns the
//! first candidate that exists on disk.
//!
//! **Luau** - relative imports (`./module`, `../module`) resolved from the
//! requiring file's directory, plus `@alias` prefixes read from `.luaurc` files
//! discovered up the directory tree.
//!
//! A [`Resolver`] owns the `.luaurc` alias cache. Create one per build so alias
//! edits are always observed; the cache never outlives the resolver.
//!
//! # Usage
//!
//! ```no_run
//! use luck_core::LuaTarget;
//! use luck_resolver::{ResolveRequest, Resolver};
//! use luck_token::Span;
//! use std::path::Path;
//!
//! let mut resolver = Resolver::new();
//! let resolved = resolver
//!     .resolve(&ResolveRequest {
//!         module: "foo",
//!         from_file: "main.lua",
//!         target: LuaTarget::Lua54,
//!         search_paths: &["./?.lua".to_string()],
//!         project_root: Path::new("."),
//!         span: Span::new(0, 3),
//!     })
//!     .unwrap();
//! assert!(resolved.path.ends_with(".lua"));
//! ```

use luck_core::diagnostics::{Diagnostic, errors};
use luck_core::types::LuaTarget;
use luck_token::Span;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

mod luau;

/// A resolved module: its normalized filesystem path and any warnings raised
/// during resolution.
#[derive(Debug)]
pub struct ResolvedModule {
    pub path: String,
    pub warnings: Vec<Diagnostic>,
}

/// One `require()` to resolve, with the context needed to locate it.
#[derive(Debug, Clone, Copy)]
pub struct ResolveRequest<'a> {
    /// The require string, e.g. `"foo.bar"`, `"./sibling"`, or `"@alias/mod"`.
    pub module: &'a str,
    /// Normalized path of the file containing the require.
    pub from_file: &'a str,
    pub target: LuaTarget,
    /// Lua template search paths. Ignored for Luau.
    pub search_paths: &'a [String],
    /// Project root that Lua templates resolve against. Ignored for Luau.
    pub project_root: &'a Path,
    /// Byte span of the require call, for diagnostics.
    pub span: Span,
}

/// Resolves `require()` strings to filesystem paths, caching the `.luaurc`
/// alias tables discovered during Luau resolution.
///
/// The cache is owned by the resolver, not global: drop the resolver (or make a
/// fresh one per build) and stale alias data goes with it.
#[derive(Debug, Default)]
pub struct Resolver {
    luaurc_cache: FxHashMap<PathBuf, Option<FxHashMap<String, String>>>,
}

impl Resolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve(
        &mut self,
        request: &ResolveRequest<'_>,
    ) -> Result<ResolvedModule, Box<Diagnostic>> {
        if request.target.is_luau() {
            self.resolve_luau(request)
        } else {
            resolve_lua(request)
        }
    }
}

fn resolve_lua(request: &ResolveRequest<'_>) -> Result<ResolvedModule, Box<Diagnostic>> {
    let transformed = request.module.replace('.', "/");
    let mut tried_paths = Vec::new();

    for template in request.search_paths {
        let relative_path = template.replace('?', &transformed);
        let abs_path = request.project_root.join(&relative_path);

        if abs_path.is_file() {
            return Ok(ResolvedModule {
                path: normalize_path(&abs_path),
                warnings: Vec::new(),
            });
        }

        tried_paths.push(relative_path);
    }

    Err(Box::new(errors::e004(
        request.from_file,
        request.span.into(),
        request.module,
        &tried_paths,
    )))
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

    fn resolve_lua_module(
        module: &str,
        from_file: &str,
        search_paths: &[String],
        project_root: &Path,
    ) -> Result<ResolvedModule, Box<Diagnostic>> {
        Resolver::new().resolve(&ResolveRequest {
            module,
            from_file,
            target: LuaTarget::Lua54,
            search_paths,
            project_root,
            span: Span::new(0, 10),
        })
    }

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
    fn resolves_module_from_first_template() {
        let dir = setup_lua_project();
        let search_paths = vec!["src/?.lua".to_string(), "src/?/init.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let result = resolve_lua_module("utils", &current_file, &search_paths, dir.path())
            .expect("resolve failed");
        assert!(result.path.ends_with("src/utils.lua"));
    }

    #[test]
    fn resolves_dotted_module_to_nested_path() {
        let dir = setup_lua_project();
        let nested = dir.path().join("src/foo");
        fs::create_dir_all(&nested).expect("failed to create dir");
        fs::write(nested.join("bar.lua"), "return {}").expect("failed to write file");

        let search_paths = vec!["src/?.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let result = resolve_lua_module("foo.bar", &current_file, &search_paths, dir.path())
            .expect("resolve failed");
        assert!(result.path.ends_with("src/foo/bar.lua"));
    }

    #[test]
    fn resolves_directory_module_via_init_template() {
        let dir = setup_lua_project();
        let search_paths = vec!["src/?.lua".to_string(), "src/?/init.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let result = resolve_lua_module("components", &current_file, &search_paths, dir.path())
            .expect("resolve failed");
        assert!(result.path.ends_with("src/components/init.lua"));
    }

    #[test]
    fn falls_back_to_later_template() {
        let dir = setup_lua_project();
        let search_paths = vec!["src/?.lua".to_string(), "lib/?.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let result = resolve_lua_module("helper", &current_file, &search_paths, dir.path())
            .expect("resolve failed");
        assert!(result.path.ends_with("lib/helper.lua"));
    }

    #[test]
    fn reports_e004_with_searched_paths_when_missing() {
        let dir = setup_lua_project();
        let search_paths = vec!["src/?.lua".to_string()];
        let current_file = normalize_path(&dir.path().join("src/main.lua"));

        let err = resolve_lua_module("nonexistent", &current_file, &search_paths, dir.path())
            .unwrap_err();
        assert_eq!(err.code, "E004");
        assert!(
            err.help
                .as_ref()
                .expect("e004 always sets help")
                .contains("src/nonexistent.lua")
        );
    }
}
