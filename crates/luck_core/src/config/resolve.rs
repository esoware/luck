//! Turning a loaded [`LuckConfig`](super::LuckConfig) into executable
//! [`BuildConfig`]s: applying
//! the selected profile, resolving entry/output paths, and expanding the
//! single-entry or multi-entry (`entries`) form.

use super::CONFIG_FILE_NAME;
use super::load::{discover_config, load_with_extends};
use crate::transform_config::TransformConfig;
use crate::types::LuaTarget;
use std::path::{Path, PathBuf};

/// Default Lua 5.x search path templates when none are configured.
pub const DEFAULT_SEARCH_PATHS: &[&str] = &["?.lua", "?/init.lua"];

/// Fully resolved configuration for a single build target, ready for execution.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub target: LuaTarget,
    pub entry: PathBuf,
    pub output: PathBuf,
    pub minify: bool,
    pub search_paths: Vec<String>,
    pub rc_dir: PathBuf,
    pub transforms: TransformConfig,
    pub preamble: Option<String>,
    pub luck_preamble: bool,
}

/// Loads or discovers the config, applies profile overrides, and returns resolved build configs.
pub fn resolve_build_config(
    config_path: Option<&Path>,
    profile: Option<&str>,
) -> Result<Vec<BuildConfig>, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("Failed to get current directory: {e}"))?;

    let (rc_path, file_config) = if let Some(path) = config_path {
        let config = load_with_extends(path)?;
        (path.to_path_buf(), config)
    } else {
        match discover_config(&cwd)? {
            Some((path, config)) => (path, config),
            None => {
                return Err(format!(
                    "No {CONFIG_FILE_NAME} found. Run from a project directory or use --config."
                ));
            }
        }
    };

    let rc_dir = rc_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cwd.clone());

    let profile_overrides = if let Some(name) = profile {
        match &file_config.profiles {
            Some(profiles) => match profiles.get(name) {
                Some(p) => Some(p.clone()),
                None => {
                    return Err(format!(
                        "Profile \"{name}\" not found in {CONFIG_FILE_NAME}"
                    ));
                }
            },
            None => return Err(format!("No profiles defined in {CONFIG_FILE_NAME}")),
        }
    } else {
        None
    };

    let minify = profile_overrides
        .as_ref()
        .and_then(|p| p.minify)
        .or(file_config.minify)
        .unwrap_or(false);
    let transforms = profile_overrides
        .as_ref()
        .and_then(|p| p.transforms.clone())
        .or(file_config.transforms.clone())
        .unwrap_or_default();

    let search_paths = file_config
        .search_paths
        .clone()
        .unwrap_or_else(|| DEFAULT_SEARCH_PATHS.iter().map(|s| s.to_string()).collect());

    let preamble = file_config.preamble.clone();
    let luck_preamble = file_config.luck_preamble.unwrap_or(false);

    if file_config.entries.is_some() && file_config.entry.is_some() {
        return Err(format!(
            "Cannot specify both \"entry\" and \"entries\" in {CONFIG_FILE_NAME}"
        ));
    }

    if let Some(entries) = &file_config.entries {
        let mut configs = Vec::new();
        for ec in entries {
            let entry = rc_dir.join(&ec.entry);
            let target = file_config.target_for_path(&entry)?;
            if !entry.is_file() {
                return Err(format!("Entry file not found: {}", entry.display()));
            }
            let output = rc_dir.join(&ec.output);
            configs.push(BuildConfig {
                target,
                entry,
                output,
                minify,
                search_paths: search_paths.clone(),
                rc_dir: rc_dir.clone(),
                transforms: transforms.clone(),
                preamble: preamble.clone(),
                luck_preamble,
            });
        }
        Ok(configs)
    } else {
        let entry_str = file_config
            .entry
            .as_deref()
            .ok_or_else(|| format!("Missing \"entry\" in {CONFIG_FILE_NAME}"))?;
        let entry = rc_dir.join(entry_str);
        let target = file_config.target_for_path(&entry)?;
        if !entry.is_file() {
            return Err(format!("Entry file not found: {}", entry.display()));
        }

        let output = if let Some(out) = &file_config.output {
            rc_dir.join(out)
        } else {
            let output_dir_str = file_config.output_dir.as_deref().ok_or_else(|| {
                format!("Missing \"output_dir\" or \"output\" in {CONFIG_FILE_NAME}")
            })?;
            let output_dir = rc_dir.join(output_dir_str);
            let entry_filename = entry
                .file_name()
                .ok_or_else(|| "Entry path has no filename".to_string())?;
            output_dir.join(entry_filename)
        };

        Ok(vec![BuildConfig {
            target,
            entry,
            output,
            minify,
            search_paths,
            rc_dir,
            transforms,
            preamble,
            luck_preamble,
        }])
    }
}
