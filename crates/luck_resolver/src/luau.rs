use luck_core::config::parse_luaurc;
use luck_core::diagnostics::{Diagnostic, errors};
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

use crate::{ResolveResult, normalize_path};

pub fn resolve_luau(
    require_string: &str,
    current_file: &str,
    require_span: std::ops::Range<usize>,
) -> Result<ResolveResult, Diagnostic> {
    if require_string.starts_with("./") || require_string.starts_with("../") {
        resolve_relative(require_string, current_file, require_span)
    } else if require_string.starts_with('@') {
        resolve_alias(require_string, current_file, require_span)
    } else {
        Err(Diagnostic::error(
            "E004",
            format!(
                "module not found: \"{require_string}\" (Luau requires must start with ./, ../, or @)"
            ),
            current_file.to_string(),
            require_span,
        )
        .with_help(
            "Luau requires must use relative paths (./foo, ../bar) or aliases (@alias/foo)"
                .to_string(),
        ))
    }
}

/// init files resolve relative to their parent's parent directory.
fn effective_dir(current_file: &Path) -> PathBuf {
    let file_name = current_file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let file_dir = current_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    if file_name == "init.luau" || file_name == "init.lua" {
        file_dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        file_dir
    }
}

fn resolve_relative(
    require_string: &str,
    current_file: &str,
    require_span: std::ops::Range<usize>,
) -> Result<ResolveResult, Diagnostic> {
    let current_path = PathBuf::from(current_file.replace('/', std::path::MAIN_SEPARATOR_STR));
    let base_dir = effective_dir(&current_path);
    let resolved_base = base_dir.join(require_string.replace('/', std::path::MAIN_SEPARATOR_STR));

    probe_luau_path(&resolved_base, current_file, require_string, require_span)
}

fn resolve_alias(
    require_string: &str,
    current_file: &str,
    require_span: std::ops::Range<usize>,
) -> Result<ResolveResult, Diagnostic> {
    let without_at = &require_string[1..];

    let (alias_name, sub_path) = match without_at.find('/') {
        Some(idx) => (&without_at[..idx], Some(&without_at[idx + 1..])),
        None => (without_at, None),
    };

    let alias_lower = alias_name.to_lowercase();
    let mut warnings = Vec::new();

    if alias_lower == "self" {
        let sub = sub_path.ok_or_else(|| {
            Diagnostic::error(
                "E004",
                "\"@self\" cannot be used alone, use @self/subpath".to_string(),
                current_file.to_string(),
                require_span.clone(),
            )
        })?;

        let current_path = PathBuf::from(current_file.replace('/', std::path::MAIN_SEPARATOR_STR));
        let self_dir = current_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        let luaurc_aliases = discover_luaurc_aliases_raw(&self_dir);
        if luaurc_aliases.keys().any(|k| k.to_lowercase() == "self") {
            // Centralized constructor - inline literal codes drift (this
            // one collided with the bundler's cycle warning).
            warnings.push(luck_core::diagnostics::errors::w004(
                current_file,
                require_span.clone(),
            ));
        }

        let resolved_base = self_dir.join(sub.replace('/', std::path::MAIN_SEPARATOR_STR));
        let mut result =
            probe_luau_path(&resolved_base, current_file, require_string, require_span)?;
        result.warnings.extend(warnings);
        return Ok(result);
    }

    let current_path = PathBuf::from(current_file.replace('/', std::path::MAIN_SEPARATOR_STR));
    let start_dir = current_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let aliases = discover_luaurc_aliases_resolved(&start_dir);

    let alias_path = aliases.get(&alias_lower).ok_or_else(|| {
        errors::e004(
            current_file,
            require_span.clone(),
            require_string,
            &[format!("alias @{alias_name} not found in any .luaurc")],
        )
    })?;

    let resolved_base = if let Some(sub) = sub_path {
        alias_path.join(sub.replace('/', std::path::MAIN_SEPARATOR_STR))
    } else {
        alias_path.clone()
    };

    let mut result = probe_luau_path(&resolved_base, current_file, require_string, require_span)?;
    result.warnings.extend(warnings);
    Ok(result)
}

/// Probes for a Luau module at base_path, checking .luau/.lua extensions
/// and init files. Reports E007 for ambiguous matches.
fn probe_luau_path(
    base_path: &Path,
    current_file: &str,
    require_string: &str,
    require_span: std::ops::Range<usize>,
) -> Result<ResolveResult, Diagnostic> {
    // Append the extension to the FULL name: `with_extension` replaces
    // everything after the last dot, so `./config.v2` would probe
    // `config.luau` instead of `config.v2.luau`.
    let mut luau_name = base_path.as_os_str().to_os_string();
    luau_name.push(".luau");
    let luau_ext = std::path::PathBuf::from(&luau_name);
    let mut lua_name = base_path.as_os_str().to_os_string();
    lua_name.push(".lua");
    let lua_ext = std::path::PathBuf::from(&lua_name);

    let luau_exists = luau_ext.is_file();
    let lua_exists = lua_ext.is_file();

    let file_match = match (luau_exists, lua_exists) {
        (true, true) => {
            return Err(errors::e007(
                current_file,
                require_span,
                &format!(
                    "both {} and {} exist",
                    normalize_path(&luau_ext),
                    normalize_path(&lua_ext)
                ),
            ));
        }
        (true, false) => Some(luau_ext),
        (false, true) => Some(lua_ext),
        (false, false) => None,
    };

    let init_luau = base_path.join("init.luau");
    let init_lua = base_path.join("init.lua");
    let init_luau_exists = init_luau.is_file();
    let init_lua_exists = init_lua.is_file();

    let dir_match = match (init_luau_exists, init_lua_exists) {
        (true, true) => {
            return Err(errors::e007(
                current_file,
                require_span,
                &format!(
                    "both {} and {} exist",
                    normalize_path(&init_luau),
                    normalize_path(&init_lua)
                ),
            ));
        }
        (true, false) => Some(init_luau),
        (false, true) => Some(init_lua),
        (false, false) => None,
    };

    if let (Some(f), Some(d)) = (&file_match, &dir_match) {
        return Err(errors::e007(
            current_file,
            require_span,
            &format!(
                "both file {} and directory init file {} exist",
                normalize_path(f),
                normalize_path(d)
            ),
        ));
    }

    if let Some(path) = file_match.or(dir_match) {
        Ok(ResolveResult {
            path: normalize_path(&path),
            warnings: Vec::new(),
        })
    } else {
        Err(errors::e004(
            current_file,
            require_span,
            require_string,
            &[
                normalize_path(&base_path.with_extension("luau")),
                normalize_path(&base_path.with_extension("lua")),
                normalize_path(&base_path.join("init.luau")),
                normalize_path(&base_path.join("init.lua")),
            ],
        ))
    }
}

/// Walks up the directory tree from `start_dir`, collecting aliases from `.luaurc` files.
/// Uses `cache` to avoid re-reading and re-parsing the same directories.
/// Returns `(directory, aliases)` pairs ordered from closest to farthest ancestor.
fn discover_luaurc_chain(
    start_dir: &Path,
    cache: &mut FxHashMap<PathBuf, Option<FxHashMap<String, String>>>,
) -> Vec<(PathBuf, FxHashMap<String, String>)> {
    let mut stack: Vec<(PathBuf, FxHashMap<String, String>)> = Vec::new();
    let mut dir = start_dir.to_path_buf();

    loop {
        let aliases = cache
            .entry(dir.clone())
            .or_insert_with(|| {
                let candidate = dir.join(".luaurc");
                if candidate.is_file()
                    && let Ok(contents) = std::fs::read_to_string(&candidate)
                    && let Ok(rc) = parse_luaurc(&contents)
                    && let Some(aliases) = rc.aliases
                {
                    Some(aliases.into_iter().collect())
                } else {
                    None
                }
            })
            .clone();

        if let Some(aliases) = aliases {
            stack.push((dir.clone(), aliases));
        }
        if !dir.pop() {
            break;
        }
    }

    stack
}

// Thread-local cache for .luaurc alias discovery, shared across calls within the same thread.
thread_local! {
    static LUAURC_CACHE: std::cell::RefCell<FxHashMap<PathBuf, Option<FxHashMap<String, String>>>> =
        std::cell::RefCell::new(FxHashMap::default());
}

/// Drop all cached `.luaurc` data. Long-lived processes that rebuild
/// (watch mode, the LSP) must call this per build, or alias edits are
/// never observed until restart.
pub fn clear_luaurc_cache() {
    LUAURC_CACHE.with(|cache| cache.borrow_mut().clear());
}

fn discover_luaurc_aliases_raw(start_dir: &Path) -> FxHashMap<String, String> {
    LUAURC_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let stack = discover_luaurc_chain(start_dir, &mut cache);

        // Merge from farthest to closest (closest wins)
        let mut merged: FxHashMap<String, String> = FxHashMap::default();
        for (_, aliases) in stack.iter().rev() {
            for (name, path) in aliases {
                merged.insert(name.to_lowercase(), path.clone());
            }
        }
        merged
    })
}

fn discover_luaurc_aliases_resolved(start_dir: &Path) -> FxHashMap<String, PathBuf> {
    LUAURC_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let stack = discover_luaurc_chain(start_dir, &mut cache);

        // Closest .luaurc wins; paths resolved relative to their .luaurc directory
        let mut merged: FxHashMap<String, PathBuf> = FxHashMap::default();
        for (rc_dir, aliases) in stack.iter().rev() {
            for (name, path_str) in aliases {
                let normalized = path_str.replace('\\', "/");
                let resolved = if Path::new(&normalized).is_absolute() {
                    PathBuf::from(&normalized)
                } else {
                    rc_dir.join(&normalized)
                };
                merged.insert(name.to_lowercase(), resolved);
            }
        }
        merged
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize_path;
    use std::fs;
    use tempfile::TempDir;

    fn setup_luau_project() -> TempDir {
        let dir = TempDir::new().expect("tempdir creation failed");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("failed to create dir");
        fs::write(src.join("main.luau"), "-- entry").expect("failed to write file");
        fs::write(src.join("utils.luau"), "return {}").expect("failed to write file");
        fs::write(src.join("helper.lua"), "return {}").expect("failed to write file");
        dir
    }

    #[test]
    fn test_luau_relative_resolve() {
        let dir = setup_luau_project();
        let current = normalize_path(&dir.path().join("src/main.luau"));

        let result = resolve_luau("./utils", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("src/utils.luau"));
    }

    #[test]
    fn test_luau_relative_resolve_lua_ext() {
        let dir = setup_luau_project();
        let current = normalize_path(&dir.path().join("src/main.luau"));

        let result = resolve_luau("./helper", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("src/helper.lua"));
    }

    #[test]
    fn test_luau_ambiguous_extension() {
        let dir = setup_luau_project();
        let src = dir.path().join("src");
        fs::write(src.join("both.luau"), "return {}").expect("failed to write file");
        fs::write(src.join("both.lua"), "return {}").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/main.luau"));
        let err = resolve_luau("./both", &current, 0..10).unwrap_err();
        assert_eq!(err.code, "E007");
    }

    #[test]
    fn test_luau_init_module() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/components");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.luau"), "return {}").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/main.luau"));
        let result = resolve_luau("./components", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("src/components/init.luau"));
    }

    #[test]
    fn test_luau_file_vs_directory_ambiguity() {
        let dir = setup_luau_project();
        let src = dir.path().join("src");
        fs::write(src.join("widget.luau"), "return {}").expect("failed to write file");
        let widget_dir = src.join("widget");
        fs::create_dir_all(&widget_dir).expect("failed to create dir");
        fs::write(widget_dir.join("init.luau"), "return {}").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/main.luau"));
        let err = resolve_luau("./widget", &current, 0..10).unwrap_err();
        assert_eq!(err.code, "E007");
    }

    #[test]
    fn test_luau_not_found() {
        let dir = setup_luau_project();
        let current = normalize_path(&dir.path().join("src/main.luau"));

        let err = resolve_luau("./nonexistent", &current, 0..10).unwrap_err();
        assert_eq!(err.code, "E004");
    }

    #[test]
    fn test_luau_unprefixed_error() {
        let dir = setup_luau_project();
        let current = normalize_path(&dir.path().join("src/main.luau"));

        let err = resolve_luau("utils", &current, 0..10).unwrap_err();
        assert_eq!(err.code, "E004");
    }

    #[test]
    fn test_luau_init_parent_directory_rule() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/components");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.luau"), "-- init").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/components/init.luau"));
        let result = resolve_luau("./utils", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("src/utils.luau"));
    }

    #[test]
    fn test_luau_alias_resolve() {
        let dir = setup_luau_project();
        let src = dir.path().join("src");

        let shared = dir.path().join("shared");
        fs::create_dir_all(&shared).expect("failed to create dir");
        fs::write(shared.join("common.luau"), "return {}").expect("failed to write file");

        let luaurc = serde_json::json!({
            "aliases": {
                "shared": "../shared"
            }
        });
        fs::write(src.join(".luaurc"), luaurc.to_string()).expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/main.luau"));
        let result = resolve_luau("@shared/common", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("shared/common.luau"));
    }

    #[test]
    fn test_luau_self_alias() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/components");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.luau"), "-- init").expect("failed to write file");
        fs::write(comp.join("button.luau"), "return {}").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/components/init.luau"));
        let result = resolve_luau("@self/button", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("src/components/button.luau"));
    }

    #[test]
    fn test_luau_self_alias_bare_error() {
        let dir = setup_luau_project();
        let current = normalize_path(&dir.path().join("src/main.luau"));

        let err = resolve_luau("@self", &current, 0..10).unwrap_err();
        assert_eq!(err.code, "E004");
    }

    #[test]
    fn test_luaurc_inheritance() {
        let dir = setup_luau_project();

        let inner = dir.path().join("src/deep/nested");
        fs::create_dir_all(&inner).expect("failed to create dir");
        fs::write(inner.join("mod.luau"), "-- entry").expect("failed to write file");

        let root_rc = serde_json::json!({
            "aliases": {
                "root_alias": "./root_lib"
            }
        });
        fs::write(dir.path().join(".luaurc"), root_rc.to_string()).expect("failed to write file");

        let src_rc = serde_json::json!({
            "aliases": {
                "src_alias": "./src_lib"
            }
        });
        fs::write(dir.path().join("src").join(".luaurc"), src_rc.to_string())
            .expect("failed to write file");

        let root_lib = dir.path().join("root_lib");
        fs::create_dir_all(&root_lib).expect("failed to create dir");
        fs::write(root_lib.join("foo.luau"), "return {}").expect("failed to write file");

        let src_lib = dir.path().join("src/src_lib");
        fs::create_dir_all(&src_lib).expect("failed to create dir");
        fs::write(src_lib.join("bar.luau"), "return {}").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/deep/nested/mod.luau"));

        let result = resolve_luau("@root_alias/foo", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("root_lib/foo.luau"));

        let result = resolve_luau("@src_alias/bar", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("src_lib/bar.luau"));
    }

    #[test]
    fn test_luau_parent_relative() {
        let dir = setup_luau_project();
        let sub = dir.path().join("src/sub");
        fs::create_dir_all(&sub).expect("failed to create dir");
        fs::write(sub.join("child.luau"), "-- child").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/sub/child.luau"));
        let result = resolve_luau("../utils", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("src/utils.luau"));
    }

    #[test]
    fn test_luau_init_lua_extension() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/legacy");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.lua"), "return {}").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/main.luau"));
        let result = resolve_luau("./legacy", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("src/legacy/init.lua"));
    }

    #[test]
    fn test_luau_alias_case_insensitive() {
        let dir = setup_luau_project();
        let src = dir.path().join("src");

        let shared = dir.path().join("shared");
        fs::create_dir_all(&shared).expect("failed to create dir");
        fs::write(shared.join("thing.luau"), "return {}").expect("failed to write file");

        let luaurc = serde_json::json!({
            "aliases": {
                "Shared": "../shared"
            }
        });
        fs::write(src.join(".luaurc"), luaurc.to_string()).expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/main.luau"));
        let result = resolve_luau("@shared/thing", &current, 0..10).expect("resolve failed");
        assert!(result.path.ends_with("shared/thing.luau"));
    }

    #[test]
    fn test_luau_init_ambiguous() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/ambig");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.luau"), "return {}").expect("failed to write file");
        fs::write(comp.join("init.lua"), "return {}").expect("failed to write file");

        let current = normalize_path(&dir.path().join("src/main.luau"));
        let err = resolve_luau("./ambig", &current, 0..10).unwrap_err();
        assert_eq!(err.code, "E007");
    }
}
