use std::fs;
use std::path::{self, Path, PathBuf};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::config::LuckConfig;

pub const CONFIG_FILE_NAME: &str = "luck.json";

/// How many files one `extends` chain may stack. Each level is a recursive
/// call, and no real project comes close.
const MAX_EXTENDS_DEPTH: usize = 64;

/// A loaded `luck.json` and the directory holding it, which roots the
/// project's relative paths and `include`/`exclude` globs.
#[derive(Debug, Clone)]
pub struct ProjectConfig {
    pub root: PathBuf,
    pub config: LuckConfig,
}

impl ProjectConfig {
    fn loaded_from(config_path: &Path, config: LuckConfig) -> Self {
        Self { root: config_directory(config_path), config }
    }
}

/// A bare file name's parent is the empty path, which can't be canonicalized
/// or watched, so it maps to `.`.
#[must_use]
pub fn config_directory(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// # Errors
///
/// Returns a message when `contents` isn't a valid config.
pub fn parse_luck_config(contents: &str) -> Result<LuckConfig, String> {
    deserialize_config(contents, CONFIG_FILE_NAME)
}

/// # Errors
///
/// Returns a message naming `file_name` when `contents` isn't a valid config.
fn deserialize_config(contents: &str, file_name: &str) -> Result<LuckConfig, String> {
    json5::from_str(contents).map_err(|e| format!("failed to parse `{file_name}` (JSON5): {e}"))
}

/// Later `extends` entries override earlier ones, and the file itself
/// overrides them all. A file reached more than once is loaded once.
///
/// # Errors
///
/// Returns a message when a file in the chain can't be read or parsed, when
/// the chain is circular or too deep, or when it escapes a `root` config's
/// directory.
pub fn load_with_extends(path: &Path) -> Result<LuckConfig, String> {
    load_layer(path, None, 0, &mut ExtendsWalk::default())
}

#[derive(Default)]
struct ExtendsWalk {
    /// The chain being loaded, so a diamond (A extends B and C, both extend
    /// D) is legal and only a path back onto the chain is a cycle.
    chain: FxHashSet<PathBuf>,
    /// Each finished file per `root` boundary it was reached under, because
    /// a boundary constrains everything below it.
    finished: FxHashMap<(PathBuf, Option<PathBuf>), LuckConfig>,
}

/// # Errors
///
/// See [`load_with_extends`].
fn load_layer(
    path: &Path,
    root_boundary: Option<&Path>,
    depth: usize,
    walk: &mut ExtendsWalk,
) -> Result<LuckConfig, String> {
    if depth == MAX_EXTENDS_DEPTH {
        return Err(format!(
            "`extends` chain is more than {MAX_EXTENDS_DEPTH} files deep at `{}`",
            path.display()
        ));
    }
    let canonical = path.canonicalize().map_err(|e| format!("`{}`: {e}", path.display()))?;
    if let Some(root_directory) = root_boundary
        && !canonical.starts_with(root_directory)
    {
        return Err(format!(
            "`extends` path `{}` escapes the root boundary at `{}`",
            path.display(),
            root_directory.display()
        ));
    }
    let finished_key = (canonical.clone(), root_boundary.map(Path::to_path_buf));
    if let Some(finished) = walk.finished.get(&finished_key) {
        return Ok(finished.clone());
    }
    if !walk.chain.insert(canonical.clone()) {
        return Err(format!("circular `extends` chain at `{}`", path.display()));
    }
    let contents = fs::read_to_string(path).map_err(|e| format!("`{}`: {e}", path.display()))?;
    let config = deserialize_config(&contents, &path.to_string_lossy())?;
    let directory = config_directory(path);
    let own_boundary = if config.root == Some(true) {
        Some(directory.canonicalize().map_err(|e| format!("`{}`: {e}", directory.display()))?)
    } else {
        None
    };
    let boundary = own_boundary.as_deref().or(root_boundary);
    let mut merged = LuckConfig::default();
    for relative_path in config.extends.iter().flatten() {
        let parent =
            load_layer(&directory.join(relative_path), boundary, depth.saturating_add(1), walk)?;
        merged = parent.merge_onto(merged);
    }
    walk.chain.remove(&canonical);
    let resolved = config.merge_onto(merged);
    walk.finished.insert(finished_key, resolved.clone());
    Ok(resolved)
}

/// `None` means discovery found none.
///
/// # Errors
///
/// Returns a message when the given or discovered config fails to load.
pub fn load_or_discover_config(
    config_path: Option<&Path>,
    start_directory: &Path,
) -> Result<Option<ProjectConfig>, String> {
    match config_path {
        Some(path) => Ok(Some(ProjectConfig::loaded_from(path, load_with_extends(path)?))),
        None => discover_config(start_directory),
    }
}

/// # Errors
///
/// Returns a message when the discovered config fails to load.
pub fn discover_config(start_directory: &Path) -> Result<Option<ProjectConfig>, String> {
    let mut directory = path::absolute(start_directory)
        .map_err(|e| format!("`{}`: {e}", start_directory.display()))?;
    loop {
        let candidate = directory.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            let config = load_with_extends(&candidate)?;
            return Ok(Some(ProjectConfig::loaded_from(&candidate, config)));
        }
        if !directory.pop() {
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_json5() {
        for bad_json in ["{ target: }", "not json at all !!!"] {
            let error = parse_luck_config(bad_json).unwrap_err();
            assert!(
                error.contains("failed to parse"),
                "error should mention parsing failure: {error}"
            );
        }
    }

    #[test]
    fn parses_json5_comments_unquoted_keys_and_trailing_commas() {
        let json5 = r#"{
            // this is a comment
            lua: "lua54",
            entry: "main.lua",
            output_dir: "dist",
            minify: true, // trailing comma
        }"#;
        let config = parse_luck_config(json5).expect("JSON5 features should parse");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.minify, Some(true));
    }

    #[test]
    fn config_directory_of_a_bare_file_name_is_the_current_directory() {
        assert_eq!(config_directory(Path::new("luck.json")), Path::new("."));
    }

    #[test]
    fn config_directory_keeps_a_named_parent() {
        assert_eq!(config_directory(Path::new("./luck.json")), Path::new("."));
        assert_eq!(config_directory(Path::new("sub/luck.json")), Path::new("sub"));
    }

    #[test]
    fn rejects_a_circular_extends_chain() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("a.json");
        let second = directory.path().join("b.json");
        fs::write(&first, r#"{ "extends": ["b.json"], "lua": "lua54" }"#).unwrap();
        fs::write(&second, r#"{ "extends": ["a.json"] }"#).unwrap();
        let message = load_with_extends(&first).unwrap_err();
        assert!(message.starts_with("circular `extends` chain"), "unexpected error: {message}");
    }

    #[test]
    fn root_config_extends_a_file_inside_its_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root_config = directory.path().join("luck.json");
        let base_config = directory.path().join("base.json");
        fs::write(&root_config, r#"{ "root": true, "extends": ["base.json"], "lua": "lua54" }"#)
            .unwrap();
        fs::write(&base_config, r#"{ "lint": { "extra_globals": ["a"] } }"#).unwrap();
        let config = load_with_extends(&root_config).expect("within-boundary extends should load");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.root, Some(true));
    }

    #[test]
    fn rejects_a_root_config_extending_outside_its_directory() {
        let directory = tempfile::tempdir().unwrap();
        let inner = directory.path().join("project");
        fs::create_dir_all(&inner).unwrap();
        let outside = directory.path().join("outside.json");
        let root_config = inner.join("luck.json");
        fs::write(&outside, r#"{ "lua": "lua53" }"#).unwrap();
        fs::write(&root_config, r#"{ "root": true, "extends": ["../outside.json"] }"#).unwrap();
        let result = load_with_extends(&root_config);
        assert!(result.is_err(), "escaping extends must error");
        let message = result.unwrap_err();
        assert!(message.contains("escapes the root boundary"), "unexpected error: {message}");
    }

    #[test]
    fn root_boundary_applies_to_a_file_already_reached_without_one() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(directory.path().join("outside.json"), "{}").unwrap();
        fs::write(project.join("shared.json"), r#"{ "extends": ["../outside.json"] }"#).unwrap();
        fs::write(project.join("root.json"), r#"{ "root": true, "extends": ["shared.json"] }"#)
            .unwrap();
        let entry = project.join("luck.json");
        fs::write(&entry, r#"{ "extends": ["shared.json", "root.json"] }"#).unwrap();
        let message = load_with_extends(&entry).unwrap_err();
        assert!(message.contains("escapes the root boundary"), "unexpected error: {message}");
    }

    #[test]
    fn extending_one_file_twice_does_not_grow_its_lists() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().join("base.json");
        let entry = directory.path().join("luck.json");
        fs::write(&base, r#"{ "include": ["a"], "lint": { "extra_globals": ["g"] } }"#).unwrap();
        fs::write(&entry, r#"{ "extends": ["base.json", "base.json"], "include": ["b"] }"#)
            .unwrap();
        let config = load_with_extends(&entry).expect("repeated extends should load");
        assert_eq!(config.include, Some(vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(config.lint.unwrap().extra_globals, vec!["g".to_owned()]);
    }

    #[test]
    fn non_root_config_extends_a_file_in_a_parent_directory() {
        let directory = tempfile::tempdir().unwrap();
        let inner = directory.path().join("project");
        fs::create_dir_all(&inner).unwrap();
        let parent = directory.path().join("shared.json");
        let child = inner.join("luck.json");
        fs::write(&parent, r#"{ "lua": "lua52" }"#).unwrap();
        fs::write(&child, r#"{ "extends": ["../shared.json"] }"#).unwrap();
        let config =
            load_with_extends(&child).expect("non-root config may extend a parent-dir config");
        assert_eq!(config.lua.as_deref(), Some("lua52"));
    }
}
