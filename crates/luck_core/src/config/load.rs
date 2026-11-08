//! Config file loading: `luck.json` (JSON5) parsing, the `extends` chain with
//! its cycle and root-boundary checks, and upward discovery.

use std::fs;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashSet;

use super::CONFIG_FILE_NAME;
use super::luck_config::LuckConfig;

/// A loaded `luck.json` and the directory holding it, which roots the
/// project's relative paths and `include`/`exclude` globs.
#[derive(Debug, Clone)]
pub struct ProjectConfig {
    pub root: PathBuf,
    pub config: LuckConfig,
}

impl ProjectConfig {
    fn loaded_from(config_path: &Path, config: LuckConfig) -> Self {
        // A path that loaded names a file, so it has a parent.
        let root = config_path.parent().map(Path::to_path_buf).unwrap_or_default();
        Self { root, config }
    }
}

/// Deserializes a `luck.json` (JSON5) string into a [`LuckConfig`].
///
/// # Errors
///
/// Returns a message when `contents` isn't a valid config.
pub fn parse_luck_config(contents: &str) -> Result<LuckConfig, String> {
    json5::from_str(contents)
        .map_err(|error| format!("failed to parse `{CONFIG_FILE_NAME}` (JSON5): {error}"))
}

/// Loads a config file and merges in its `extends` chain.
///
/// Later `extends` entries override earlier ones, and the file itself
/// overrides them all.
///
/// # Errors
///
/// Returns a message when a file in the chain can't be read or parsed, when
/// the chain is circular, or when it escapes a `root` config's directory.
pub fn load_with_extends(path: &Path) -> Result<LuckConfig, String> {
    let mut visited_paths = FxHashSet::default();
    load_with_extends_inner(path, &mut visited_paths)
}

/// # Errors
///
/// See [`load_with_extends`].
fn load_with_extends_inner(
    path: &Path,
    visited_paths: &mut FxHashSet<PathBuf>,
) -> Result<LuckConfig, String> {
    let canonical =
        path.canonicalize().map_err(|error| format!("`{}`: {error}", path.display()))?;
    // `visited_paths` holds only the active chain, so a diamond (A extends B and C,
    // both extend D) is legal and only a path back onto the chain is a cycle.
    if !visited_paths.insert(canonical.clone()) {
        return Err(format!("circular `extends` chain at `{}`", path.display()));
    }
    let contents =
        fs::read_to_string(path).map_err(|error| format!("`{}`: {error}", path.display()))?;
    let config = parse_luck_config(&contents)?;
    let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut merged = LuckConfig::default();
    if let Some(parents) = &config.extends {
        // A `root: true` config's extends must stay inside its own directory.
        let root_boundary = if config.root == Some(true) {
            Some(
                directory
                    .canonicalize()
                    .map_err(|error| format!("`{}`: {error}", directory.display()))?,
            )
        } else {
            None
        };
        for relative_path in parents {
            let parent_path = directory.join(relative_path);
            if let Some(root_directory) = &root_boundary {
                let canonical_parent = parent_path
                    .canonicalize()
                    .map_err(|error| format!("`{}`: {error}", parent_path.display()))?;
                if !canonical_parent.starts_with(root_directory) {
                    return Err(format!(
                        "`extends` path `{relative_path}` escapes the root boundary at `{}`",
                        root_directory.display()
                    ));
                }
            }
            let parent = load_with_extends_inner(&parent_path, visited_paths)?;
            merged = parent.merge_onto(merged); // later extends override earlier
        }
    }
    visited_paths.remove(&canonical);
    Ok(config.merge_onto(merged)) // current file overrides all parents
}

/// Loads the config at `config_path` when given, or else discovers the nearest
/// one walking up from `start_directory`. `None` means discovery found none.
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

/// Walks up from `start_directory` to the nearest `luck.json` and loads it.
///
/// # Errors
///
/// Returns a message when the discovered config fails to load.
pub fn discover_config(start_directory: &Path) -> Result<Option<ProjectConfig>, String> {
    let mut directory = start_directory.to_path_buf();
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
