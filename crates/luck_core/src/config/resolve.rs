//! Turning a loaded [`LuckConfig`] into [`BuildConfig`]s: applying the
//! selected profile, resolving entry and output paths, and expanding the
//! single-entry or multi-entry (`entries`) form.

use std::env;
use std::path::{Path, PathBuf};

use super::CONFIG_FILE_NAME;
use super::dynamic_require::DynamicRequire;
use super::load::{ProjectConfig, load_or_discover_config};
use super::luck_config::{LuckConfig, ProfileOverrides};
use super::transform_config::TransformConfig;
use crate::lua_target::LuaTarget;

/// The resolved settings for building one entry.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub target: LuaTarget,
    pub entry: PathBuf,
    pub output: PathBuf,
    pub minify: bool,
    pub dynamic_require: DynamicRequire,
    pub search_paths: Vec<String>,
    pub project_root: PathBuf,
    pub transforms: TransformConfig,
    pub preamble: Option<String>,
    pub luck_preamble: bool,
}

/// Loads or discovers `luck.json`, applies `profile`, and resolves one
/// [`BuildConfig`] per entry.
///
/// # Errors
///
/// Returns a message when no config is found or it fails to load, when
/// `profile` isn't defined, or when the entry, output, or dialect settings are
/// missing or invalid.
pub fn resolve_build_config(
    config_path: Option<&Path>,
    profile: Option<&str>,
) -> Result<Vec<BuildConfig>, String> {
    let current_directory =
        env::current_dir().map_err(|error| format!("failed to get current directory: {error}"))?;

    let ProjectConfig { root: project_root, config } =
        load_or_discover_config(config_path, &current_directory)?.ok_or_else(|| {
            format!(
                "no `{CONFIG_FILE_NAME}` found, so run from a project directory or use `--config`"
            )
        })?;

    let profile_overrides = select_profile(&config, profile)?;
    let minify =
        profile_overrides.and_then(|overrides| overrides.minify).or(config.minify).unwrap_or(false);
    let transforms = profile_overrides
        .and_then(|overrides| overrides.transforms)
        .or(config.transforms)
        .unwrap_or_default();
    let dynamic_require = config.dynamic_require.unwrap_or_default();
    let search_paths = config.search_paths_or_default();
    let luck_preamble = config.luck_preamble.unwrap_or(false);

    let build_config = |target, entry, output| BuildConfig {
        target,
        entry,
        output,
        minify,
        dynamic_require,
        search_paths: search_paths.clone(),
        project_root: project_root.clone(),
        transforms,
        preamble: config.preamble.clone(),
        luck_preamble,
    };
    let resolve_entry = |relative_entry: &str| -> Result<(LuaTarget, PathBuf), String> {
        let entry = project_root.join(relative_entry);
        let target = config.target_for_path(&entry)?;
        if !entry.is_file() {
            return Err(format!("entry file not found: `{}`", entry.display()));
        }
        Ok((target, entry))
    };

    match (&config.entries, &config.entry) {
        (Some(_), Some(_)) => {
            Err(format!("cannot specify both `entry` and `entries` in `{CONFIG_FILE_NAME}`"))
        }
        (Some(entries), None) => {
            if entries.is_empty() {
                return Err(format!(
                    "`entries` in `{CONFIG_FILE_NAME}` is empty, so list at least one entry to build"
                ));
            }
            entries
                .iter()
                .map(|entry_config| {
                    let (target, entry) = resolve_entry(&entry_config.entry)?;
                    Ok(build_config(target, entry, project_root.join(&entry_config.output)))
                })
                .collect()
        }
        (None, None) => Err(format!("missing `entry` in `{CONFIG_FILE_NAME}`")),
        (None, Some(relative_entry)) => {
            let (target, entry) = resolve_entry(relative_entry)?;
            let output = if let Some(output_path) = &config.output {
                project_root.join(output_path)
            } else {
                let relative_output_directory = config.output_dir.as_deref().ok_or_else(|| {
                    format!("missing `output_dir` or `output` in `{CONFIG_FILE_NAME}`")
                })?;
                let entry_filename =
                    entry.file_name().ok_or_else(|| "entry path has no filename".to_owned())?;
                project_root.join(relative_output_directory).join(entry_filename)
            };
            Ok(vec![build_config(target, entry, output)])
        }
    }
}

/// Looks up the overrides for `profile`, if one was requested.
///
/// # Errors
///
/// Returns a message when the config doesn't define `profile`.
fn select_profile(
    config: &LuckConfig,
    profile: Option<&str>,
) -> Result<Option<ProfileOverrides>, String> {
    let Some(name) = profile else {
        return Ok(None);
    };
    let profiles = config
        .profiles
        .as_ref()
        .ok_or_else(|| format!("no profiles defined in `{CONFIG_FILE_NAME}`"))?;
    let overrides = profiles
        .get(name)
        .ok_or_else(|| format!("profile `{name}` not found in `{CONFIG_FILE_NAME}`"))?;
    Ok(Some(*overrides))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn rejects_empty_entries() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join(CONFIG_FILE_NAME);
        fs::write(&config_path, r#"{"entries": []}"#).expect("write config");

        let error =
            resolve_build_config(Some(&config_path), None).expect_err("empty entries resolved");
        assert!(error.contains("`entries`"), "{error}");
    }
}
