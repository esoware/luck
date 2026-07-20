//! Reading config files off disk: `luck.json` (JSON5) parsing, the recursive
//! `extends` chain with cycle and root-boundary checks, upward discovery, and
//! `.luaurc` parsing.

use super::CONFIG_FILE_NAME;
use super::schema::LuckConfig;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Parsed `.luaurc` file with optional aliases for Luau module resolution.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LuauRc {
    pub language_mode: Option<String>,
    pub aliases: Option<HashMap<String, String>>,
}

/// Deserializes a `luck.json` (JSON5) string into a [`LuckConfig`].
pub fn parse_luck_config(contents: &str) -> Result<LuckConfig, String> {
    json5::from_str(contents)
        .map_err(|e| format!("Failed to parse {CONFIG_FILE_NAME} (JSON5): {e}"))
}

/// Deserializes a `.luaurc` (JSON5) string into a [`LuauRc`].
pub fn parse_luaurc(contents: &str) -> Result<LuauRc, String> {
    json5::from_str(contents).map_err(|e| format!("Failed to parse .luaurc: {e}"))
}

/// Loads a config file and recursively applies its `extends` chain, merging
/// each parent base-first. Cycles are detected and errored.
pub fn load_with_extends(path: &Path) -> Result<LuckConfig, String> {
    let mut visited = HashSet::new();
    load_with_extends_inner(path, &mut visited)
}

fn load_with_extends_inner(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<LuckConfig, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // `visited` tracks the ACTIVE chain, not everything ever loaded - a
    // diamond (A extends B and C, both extend D) is legal; only a path
    // back onto the current chain is a cycle. Popped before returning.
    if !visited.insert(canonical.clone()) {
        return Err(format!("circular extends chain at {}", path.display()));
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config = parse_luck_config(&contents)?;
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut merged = LuckConfig::default();
    if let Some(parents) = &config.extends {
        // A `root: true` config is the top of its project; its extends must
        // not escape its own directory subtree.
        let root_boundary = if config.root == Some(true) {
            Some(
                dir.canonicalize()
                    .map_err(|e| format!("{}: {e}", dir.display()))?,
            )
        } else {
            None
        };
        for rel in parents {
            let parent_path = dir.join(rel);
            if let Some(root_dir) = &root_boundary {
                let target = parent_path
                    .canonicalize()
                    .map_err(|e| format!("{}: {e}", parent_path.display()))?;
                if !target.starts_with(root_dir) {
                    return Err(format!(
                        "extends path \"{rel}\" escapes the root boundary at {}",
                        root_dir.display()
                    ));
                }
            }
            let parent = load_with_extends_inner(&parent_path, visited)?;
            merged = parent.merge_onto(merged); // later extends override earlier
        }
    }
    visited.remove(&canonical);
    Ok(config.merge_onto(merged)) // current file overrides all parents
}

/// Walks up from `start_dir` looking for a `luck.json` file.
pub fn discover_config(start_dir: &Path) -> Result<Option<(PathBuf, LuckConfig)>, String> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            let config = load_with_extends(&candidate)?;
            return Ok(Some((candidate, config)));
        }
        if !dir.pop() {
            return Ok(None);
        }
    }
}
