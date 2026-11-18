use std::path::{Path, PathBuf};

use luck_core::LuaTarget;
use luck_minifier::TransformConfig;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::config::{EntryConfig, ProfileOverrides};
use crate::load::CONFIG_FILE_NAME;
use crate::path::canonicalize_existing;

/// One bundle `luck build` writes, with the settings its profile gives it.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub target: LuaTarget,
    pub entry: PathBuf,
    pub output: PathBuf,
    pub minify: bool,
    pub transforms: TransformConfig,
    pub preamble: Option<String>,
    pub luck_preamble: bool,
}

/// The keys only `luck build` reads.
#[derive(Debug, Default)]
pub(crate) struct BuildKeys {
    pub entry: Option<String>,
    pub output_dir: Option<String>,
    pub output: Option<String>,
    pub entries: Option<Vec<EntryConfig>>,
    pub minify: bool,
    pub profiles: FxHashMap<String, ProfileOverrides>,
    pub preamble: Option<String>,
    pub luck_preamble: bool,
}

impl BuildKeys {
    /// The builds `profile` selects, with entries and outputs under `root`,
    /// and `project_transforms` where the profile sets none.
    ///
    /// # Errors
    ///
    /// Returns a message when `profile` isn't defined, when there's no entry
    /// or output, when an entry isn't a file, or when an output collides with
    /// an entry or another output.
    pub(crate) fn builds(
        &self,
        root: &Path,
        project_transforms: TransformConfig,
        target_for: impl Fn(&Path) -> LuaTarget,
        profile: Option<&str>,
    ) -> Result<Vec<BuildConfig>, String> {
        let profile_overrides = match profile {
            Some(name) => Some(self.profiles.get(name).ok_or_else(|| {
                format!("profile `{name}` isn't defined in `{CONFIG_FILE_NAME}`")
            })?),
            None => None,
        };
        let minify =
            profile_overrides.and_then(|overrides| overrides.minify).unwrap_or(self.minify);
        let transforms = profile_overrides
            .and_then(|overrides| overrides.transforms)
            .unwrap_or(project_transforms);
        let entry_build = |relative_entry: &str, output: PathBuf| {
            let entry = root.join(relative_entry);
            if !entry.is_file() {
                return Err(format!("entry file not found: `{}`", entry.display()));
            }
            Ok(BuildConfig {
                target: target_for(&entry),
                entry,
                output,
                minify,
                transforms,
                preamble: self.preamble.clone(),
                luck_preamble: self.luck_preamble,
            })
        };

        let builds: Vec<BuildConfig> = if let Some(entries) = &self.entries {
            entries
                .iter()
                .map(|entry| entry_build(&entry.entry, root.join(&entry.output)))
                .collect::<Result<_, String>>()?
        } else {
            let relative_entry = self
                .entry
                .as_deref()
                .ok_or_else(|| format!("missing `entry` in `{CONFIG_FILE_NAME}`"))?;
            let output = if let Some(output) = &self.output {
                root.join(output)
            } else {
                let output_directory = self.output_dir.as_deref().ok_or_else(|| {
                    format!("missing `output_dir` or `output` in `{CONFIG_FILE_NAME}`")
                })?;
                let file_name = Path::new(relative_entry)
                    .file_name()
                    .ok_or_else(|| format!("entry `{relative_entry}` names no file"))?;
                root.join(output_directory).join(file_name)
            };
            vec![entry_build(relative_entry, output)?]
        };
        check_output_paths(
            builds.iter().map(|build| &build.entry),
            builds.iter().map(|build| &build.output),
        )?;
        Ok(builds)
    }
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

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
