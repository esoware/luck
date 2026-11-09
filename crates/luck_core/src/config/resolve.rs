//! Turning a loaded [`LuckConfig`] into [`BuildConfig`]s: applying the
//! selected profile, resolving entry and output paths, and expanding the
//! single-entry or multi-entry (`entries`) form.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashSet;

use super::CONFIG_FILE_NAME;
use super::dynamic_require::DynamicRequire;
use super::load::ProjectConfig;
use super::luck_config::{LuckConfig, ProfileOverrides};
use super::transform_config::TransformConfig;
use crate::lua_target::LuaTarget;
use crate::path::canonicalize_existing;

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

/// Applies `profile` to a loaded project and resolves one [`BuildConfig`] per
/// entry.
///
/// # Errors
///
/// Returns a message when `profile` isn't defined, when the entry, output, or
/// dialect settings are missing or invalid, or when an output collides with an
/// entry or another output.
pub fn resolve_build_config(
    project: &ProjectConfig,
    profile: Option<&str>,
) -> Result<Vec<BuildConfig>, String> {
    let ProjectConfig { root: project_root, config } = project;

    let profile_overrides = select_profile(config, profile)?;
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

    let builds: Vec<BuildConfig> = match (&config.entries, &config.entry) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "cannot specify both `entry` and `entries` in `{CONFIG_FILE_NAME}`"
            ));
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
                .collect::<Result<_, String>>()?
        }
        (None, None) => return Err(format!("missing `entry` in `{CONFIG_FILE_NAME}`")),
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
            vec![build_config(target, entry, output)]
        }
    };
    check_output_paths(
        builds.iter().map(|build| &build.entry),
        builds.iter().map(|build| &build.output),
    )?;
    Ok(builds)
}

/// Rejects `outputs` that would replace one of `inputs` or each other. Paths
/// compare by where they really lead, so two spellings of one path collide
/// and an output not on disk yet is still checked.
///
/// # Errors
///
/// Returns a message naming the first output that collides.
pub fn check_output_paths<Inputs, Outputs>(inputs: Inputs, outputs: Outputs) -> Result<(), String>
where
    Inputs: IntoIterator,
    Inputs::Item: AsRef<Path>,
    Outputs: IntoIterator,
    Outputs::Item: AsRef<Path>,
{
    let read_paths: FxHashSet<PathBuf> =
        inputs.into_iter().map(|input| canonicalize_existing(input.as_ref())).collect();
    let mut written_paths = FxHashSet::default();
    for output in outputs {
        let output_path = output.as_ref();
        let canonical = canonicalize_existing(output_path);
        if read_paths.contains(&canonical) {
            return Err(format!(
                "output `{}` would overwrite a source file of the build",
                output_path.display()
            ));
        }
        if !written_paths.insert(canonical) {
            return Err(format!(
                "output `{}` is written more than once, so one result would replace another",
                output_path.display()
            ));
        }
    }
    Ok(())
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
    use crate::config::parse_luck_config;

    #[test]
    fn rejects_empty_entries() {
        let config = parse_luck_config(r#"{"entries": []}"#).expect("config parses");
        let project = ProjectConfig { root: PathBuf::from("."), config };

        let error = resolve_build_config(&project, None).expect_err("empty entries resolved");
        assert!(error.contains("`entries`"), "{error}");
    }

    #[test]
    fn output_paths_collide_through_a_directory_not_on_disk() {
        let directory = tempfile::tempdir().expect("tempdir");
        let entry = directory.path().join("main.lua");
        fs::write(&entry, "return 1").expect("write entry");
        let same_as_entry = directory.path().join("gone/../main.lua");
        assert!(check_output_paths([&entry], [&same_as_entry]).is_err());
        let spare = directory.path().join("out/bundle.lua");
        check_output_paths([&entry], [&spare]).expect("a spare output doesn't collide");
        assert!(check_output_paths([&entry], [&spare, &spare]).is_err());
    }
}
