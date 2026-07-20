use luck_core::config::parse_luaurc;
use luck_core::diagnostics::{Diagnostic, errors};
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

use crate::{ResolveRequest, ResolvedModule, Resolver, normalize_path, normalize_path_str};

impl Resolver {
    pub(crate) fn resolve_luau(
        &mut self,
        request: &ResolveRequest<'_>,
    ) -> Result<ResolvedModule, Diagnostic> {
        if request.module.starts_with("./") || request.module.starts_with("../") {
            self.resolve_relative(request)
        } else if request.module.starts_with('@') {
            self.resolve_alias(request)
        } else {
            Err(errors::e004_luau_scheme(
                request.from_file,
                request.span.into(),
                request.module,
            ))
        }
    }

    fn resolve_relative(&self, request: &ResolveRequest<'_>) -> Result<ResolvedModule, Diagnostic> {
        let current_path = os_path(request.from_file);
        let base_dir = effective_dir(&current_path);
        let resolved_base = base_dir.join(os_path(request.module));

        probe_luau_path(&resolved_base, request)
    }

    fn resolve_alias(
        &mut self,
        request: &ResolveRequest<'_>,
    ) -> Result<ResolvedModule, Diagnostic> {
        let without_at = &request.module[1..];
        let (alias_name, sub_path) = match without_at.find('/') {
            Some(idx) => (&without_at[..idx], Some(&without_at[idx + 1..])),
            None => (without_at, None),
        };
        let alias_lower = alias_name.to_lowercase();

        let current_path = os_path(request.from_file);
        let start_dir = current_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        if alias_lower == "self" {
            return self.resolve_self(request, sub_path, &start_dir);
        }

        let aliases = self.luaurc_aliases(&start_dir);
        let alias_path = aliases.get(&alias_lower).ok_or_else(|| {
            errors::e004(
                request.from_file,
                request.span.into(),
                request.module,
                &[format!("alias @{alias_name} not found in any .luaurc")],
            )
        })?;

        let resolved_base = match sub_path {
            Some(sub) => alias_path.join(os_path(sub)),
            None => alias_path.clone(),
        };

        probe_luau_path(&resolved_base, request)
    }

    /// `@self` resolves against the requiring file's own directory. A `.luaurc`
    /// alias literally named `self` is shadowed by this built-in, so warn (W004).
    fn resolve_self(
        &mut self,
        request: &ResolveRequest<'_>,
        sub_path: Option<&str>,
        self_dir: &Path,
    ) -> Result<ResolvedModule, Diagnostic> {
        let sub = sub_path.ok_or_else(|| {
            errors::e004_self_needs_subpath(request.from_file, request.span.into())
        })?;

        let mut warnings = Vec::new();
        if self.luaurc_aliases(self_dir).contains_key("self") {
            warnings.push(errors::w004(request.from_file, request.span.into()));
        }

        let resolved_base = self_dir.join(os_path(sub));
        let mut result = probe_luau_path(&resolved_base, request)?;
        result.warnings.extend(warnings);
        Ok(result)
    }

    /// Resolved `@alias` -> directory map for `start_dir`, keyed by lowercased
    /// alias name. Closer `.luaurc` files win; relative alias targets resolve
    /// against the `.luaurc` that declared them.
    fn luaurc_aliases(&mut self, start_dir: &Path) -> FxHashMap<String, PathBuf> {
        let mut merged: FxHashMap<String, PathBuf> = FxHashMap::default();
        // Walk farthest-to-closest so the closest definition overwrites last.
        for (rc_dir, aliases) in self.luaurc_chain(start_dir).into_iter().rev() {
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
    }

    /// `.luaurc` alias tables from `start_dir` up to the filesystem root,
    /// closest ancestor first. Reads and parses each directory at most once.
    fn luaurc_chain(&mut self, start_dir: &Path) -> Vec<(PathBuf, FxHashMap<String, String>)> {
        let mut chain = Vec::new();
        let mut dir = start_dir.to_path_buf();

        loop {
            let aliases = self
                .luaurc_cache
                .entry(dir.clone())
                .or_insert_with(|| read_luaurc_aliases(&dir))
                .clone();

            if let Some(aliases) = aliases {
                chain.push((dir.clone(), aliases));
            }
            if !dir.pop() {
                break;
            }
        }

        chain
    }
}

/// Reads and parses `dir/.luaurc`, returning its aliases if the file exists and
/// declares any.
fn read_luaurc_aliases(dir: &Path) -> Option<FxHashMap<String, String>> {
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
}

/// An `init.lua`/`init.luau` file resolves relative imports against its parent's
/// parent directory, matching Roblox's resolver.
fn effective_dir(current_file: &Path) -> PathBuf {
    let file_name = current_file
        .file_name()
        .and_then(|name| name.to_str())
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

/// Probes for a Luau module at `base_path`, trying `.luau`/`.lua` extensions and
/// then `init.luau`/`init.lua`. Reports E007 when a choice is ambiguous.
fn probe_luau_path(
    base_path: &Path,
    request: &ResolveRequest<'_>,
) -> Result<ResolvedModule, Diagnostic> {
    // Append the extension to the FULL name: `with_extension` replaces
    // everything after the last dot, so `./config.v2` would probe
    // `config.luau` instead of `config.v2.luau`.
    let luau_ext = append_extension(base_path, ".luau");
    let lua_ext = append_extension(base_path, ".lua");

    let file_match = match (luau_ext.is_file(), lua_ext.is_file()) {
        (true, true) => {
            return Err(ambiguous(request, &luau_ext, &lua_ext));
        }
        (true, false) => Some(luau_ext),
        (false, true) => Some(lua_ext),
        (false, false) => None,
    };

    let init_luau = base_path.join("init.luau");
    let init_lua = base_path.join("init.lua");

    let dir_match = match (init_luau.is_file(), init_lua.is_file()) {
        (true, true) => {
            return Err(ambiguous(request, &init_luau, &init_lua));
        }
        (true, false) => Some(init_luau),
        (false, true) => Some(init_lua),
        (false, false) => None,
    };

    if let (Some(file), Some(dir)) = (&file_match, &dir_match) {
        return Err(errors::e007(
            request.from_file,
            request.span.into(),
            &format!(
                "both file {} and directory init file {} exist",
                normalize_path_str(file),
                normalize_path_str(dir)
            ),
        ));
    }

    match file_match.or(dir_match) {
        Some(path) => Ok(ResolvedModule {
            path: normalize_path(&path),
            warnings: Vec::new(),
        }),
        None => Err(errors::e004(
            request.from_file,
            request.span.into(),
            request.module,
            &[
                normalize_path_str(&append_extension(base_path, ".luau")),
                normalize_path_str(&append_extension(base_path, ".lua")),
                normalize_path_str(&base_path.join("init.luau")),
                normalize_path_str(&base_path.join("init.lua")),
            ],
        )),
    }
}

fn ambiguous(request: &ResolveRequest<'_>, first: &Path, second: &Path) -> Diagnostic {
    errors::e007(
        request.from_file,
        request.span.into(),
        &format!(
            "both {} and {} exist",
            normalize_path_str(first),
            normalize_path_str(second)
        ),
    )
}

fn append_extension(base_path: &Path, extension: &str) -> PathBuf {
    let mut name = base_path.as_os_str().to_os_string();
    name.push(extension);
    PathBuf::from(name)
}

fn os_path(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', std::path::MAIN_SEPARATOR_STR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize_path_str;
    use luck_core::types::LuaTarget;
    use luck_token::Span;
    use std::fs;
    use tempfile::TempDir;

    fn resolve_luau_module(module: &str, from_file: &str) -> Result<ResolvedModule, Diagnostic> {
        Resolver::new().resolve(&ResolveRequest {
            module,
            from_file,
            target: LuaTarget::Luau,
            search_paths: &[],
            project_root: Path::new("."),
            span: Span::new(0, 10),
        })
    }

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
    fn resolves_relative_luau_file() {
        let dir = setup_luau_project();
        let current = normalize_path_str(&dir.path().join("src/main.luau"));

        let result = resolve_luau_module("./utils", &current).expect("resolve failed");
        assert!(result.path.ends_with("src/utils.luau"));
    }

    #[test]
    fn prefers_lua_when_only_lua_exists() {
        let dir = setup_luau_project();
        let current = normalize_path_str(&dir.path().join("src/main.luau"));

        let result = resolve_luau_module("./helper", &current).expect("resolve failed");
        assert!(result.path.ends_with("src/helper.lua"));
    }

    #[test]
    fn flags_ambiguous_extension() {
        let dir = setup_luau_project();
        let src = dir.path().join("src");
        fs::write(src.join("both.luau"), "return {}").expect("failed to write file");
        fs::write(src.join("both.lua"), "return {}").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/main.luau"));
        let err = resolve_luau_module("./both", &current).unwrap_err();
        assert_eq!(err.code, "E007");
    }

    #[test]
    fn resolves_directory_module_via_init() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/components");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.luau"), "return {}").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/main.luau"));
        let result = resolve_luau_module("./components", &current).expect("resolve failed");
        assert!(result.path.ends_with("src/components/init.luau"));
    }

    #[test]
    fn flags_file_versus_directory_ambiguity() {
        let dir = setup_luau_project();
        let src = dir.path().join("src");
        fs::write(src.join("widget.luau"), "return {}").expect("failed to write file");
        let widget_dir = src.join("widget");
        fs::create_dir_all(&widget_dir).expect("failed to create dir");
        fs::write(widget_dir.join("init.luau"), "return {}").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/main.luau"));
        let err = resolve_luau_module("./widget", &current).unwrap_err();
        assert_eq!(err.code, "E007");
    }

    #[test]
    fn reports_e004_when_relative_missing() {
        let dir = setup_luau_project();
        let current = normalize_path_str(&dir.path().join("src/main.luau"));

        let err = resolve_luau_module("./nonexistent", &current).unwrap_err();
        assert_eq!(err.code, "E004");
    }

    #[test]
    fn rejects_unprefixed_require() {
        let dir = setup_luau_project();
        let current = normalize_path_str(&dir.path().join("src/main.luau"));

        let err = resolve_luau_module("utils", &current).unwrap_err();
        assert_eq!(err.code, "E004");
    }

    #[test]
    fn init_file_resolves_from_parent_parent() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/components");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.luau"), "-- init").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/components/init.luau"));
        let result = resolve_luau_module("./utils", &current).expect("resolve failed");
        assert!(result.path.ends_with("src/utils.luau"));
    }

    #[test]
    fn resolves_alias_from_luaurc() {
        let dir = setup_luau_project();
        let src = dir.path().join("src");

        let shared = dir.path().join("shared");
        fs::create_dir_all(&shared).expect("failed to create dir");
        fs::write(shared.join("common.luau"), "return {}").expect("failed to write file");

        let luaurc = serde_json::json!({ "aliases": { "shared": "../shared" } });
        fs::write(src.join(".luaurc"), luaurc.to_string()).expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/main.luau"));
        let result = resolve_luau_module("@shared/common", &current).expect("resolve failed");
        assert!(result.path.ends_with("shared/common.luau"));
    }

    #[test]
    fn resolves_self_alias() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/components");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.luau"), "-- init").expect("failed to write file");
        fs::write(comp.join("button.luau"), "return {}").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/components/init.luau"));
        let result = resolve_luau_module("@self/button", &current).expect("resolve failed");
        assert!(result.path.ends_with("src/components/button.luau"));
    }

    #[test]
    fn rejects_bare_self_alias() {
        let dir = setup_luau_project();
        let current = normalize_path_str(&dir.path().join("src/main.luau"));

        let err = resolve_luau_module("@self", &current).unwrap_err();
        assert_eq!(err.code, "E004");
    }

    #[test]
    fn closest_luaurc_wins_over_ancestor() {
        let dir = setup_luau_project();

        let inner = dir.path().join("src/deep/nested");
        fs::create_dir_all(&inner).expect("failed to create dir");
        fs::write(inner.join("mod.luau"), "-- entry").expect("failed to write file");

        let root_rc = serde_json::json!({ "aliases": { "root_alias": "./root_lib" } });
        fs::write(dir.path().join(".luaurc"), root_rc.to_string()).expect("failed to write file");

        let src_rc = serde_json::json!({ "aliases": { "src_alias": "./src_lib" } });
        fs::write(dir.path().join("src").join(".luaurc"), src_rc.to_string())
            .expect("failed to write file");

        let root_lib = dir.path().join("root_lib");
        fs::create_dir_all(&root_lib).expect("failed to create dir");
        fs::write(root_lib.join("foo.luau"), "return {}").expect("failed to write file");

        let src_lib = dir.path().join("src/src_lib");
        fs::create_dir_all(&src_lib).expect("failed to create dir");
        fs::write(src_lib.join("bar.luau"), "return {}").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/deep/nested/mod.luau"));

        let result = resolve_luau_module("@root_alias/foo", &current).expect("resolve failed");
        assert!(result.path.ends_with("root_lib/foo.luau"));

        let result = resolve_luau_module("@src_alias/bar", &current).expect("resolve failed");
        assert!(result.path.ends_with("src_lib/bar.luau"));
    }

    #[test]
    fn resolves_parent_relative() {
        let dir = setup_luau_project();
        let sub = dir.path().join("src/sub");
        fs::create_dir_all(&sub).expect("failed to create dir");
        fs::write(sub.join("child.luau"), "-- child").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/sub/child.luau"));
        let result = resolve_luau_module("../utils", &current).expect("resolve failed");
        assert!(result.path.ends_with("src/utils.luau"));
    }

    #[test]
    fn resolves_directory_via_init_lua() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/legacy");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.lua"), "return {}").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/main.luau"));
        let result = resolve_luau_module("./legacy", &current).expect("resolve failed");
        assert!(result.path.ends_with("src/legacy/init.lua"));
    }

    #[test]
    fn matches_alias_case_insensitively() {
        let dir = setup_luau_project();
        let src = dir.path().join("src");

        let shared = dir.path().join("shared");
        fs::create_dir_all(&shared).expect("failed to create dir");
        fs::write(shared.join("thing.luau"), "return {}").expect("failed to write file");

        let luaurc = serde_json::json!({ "aliases": { "Shared": "../shared" } });
        fs::write(src.join(".luaurc"), luaurc.to_string()).expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/main.luau"));
        let result = resolve_luau_module("@shared/thing", &current).expect("resolve failed");
        assert!(result.path.ends_with("shared/thing.luau"));
    }

    #[test]
    fn flags_ambiguous_init_extension() {
        let dir = setup_luau_project();
        let comp = dir.path().join("src/ambig");
        fs::create_dir_all(&comp).expect("failed to create dir");
        fs::write(comp.join("init.luau"), "return {}").expect("failed to write file");
        fs::write(comp.join("init.lua"), "return {}").expect("failed to write file");

        let current = normalize_path_str(&dir.path().join("src/main.luau"));
        let err = resolve_luau_module("./ambig", &current).unwrap_err();
        assert_eq!(err.code, "E007");
    }
}
