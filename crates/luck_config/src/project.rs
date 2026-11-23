use alloc::sync::Arc;
use core::mem;
use std::path::{Path, PathBuf};

use globset::{Candidate, GlobSet};
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_core::LuaTarget;
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_formatter::FormatOptions;
use luck_linter::LintConfig;
use luck_minifier::TransformConfig;
use luck_token::LuaVersion;

use crate::build::{BuildConfig, BuildKeys};
use crate::config::{Entries, LuckConfig, OverrideConfig};
use crate::editorconfig::Editorconfigs;
use crate::filter::ProjectFilter;
use crate::format_config::FormatConfig;
use crate::glob::compiled;
use crate::load::{self, Loaded, config_directory, in_file};
use crate::path::canonicalize_existing;

/// A `luck.json` loaded through its `extends` chain and validated, or the
/// defaults where there's none, and the settings it gives each file.
#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    files: Vec<PathBuf>,
    lua: LuaTarget,
    luau: LuaTarget,
    filter: ProjectFilter,
    format: FormatConfig,
    lint: Arc<LintConfig>,
    overrides: Vec<Override>,
    search_paths: Vec<String>,
    dynamic_require: DynamicRequire,
    transforms: TransformConfig,
    build: BuildKeys,
}

#[derive(Debug)]
struct Override {
    files: GlobSet,
    exclude_files: GlobSet,
    format: Option<FormatConfig>,
    lint: Option<LintConfig>,
}

impl Project {
    /// The project of the `luck.json` at `config_path`, rooted at its
    /// directory.
    ///
    /// # Errors
    ///
    /// Returns E010 when `config_path` can't be read, and E014 when it or a
    /// file it extends isn't a valid config, each in the file at fault and
    /// placed where the problem is.
    pub fn load(config_path: &Path) -> Result<Self, FileDiagnostics> {
        let Loaded { config, files } = load::load(config_path)?;
        let invalid = |problem: &str| in_file(config_path, Diagnostic::e014(None, problem));
        #[expect(
            clippy::unneeded_field_pattern,
            reason = "every key is named, so one `luck.json` gains can't go unread here"
        )]
        let LuckConfig {
            lua,
            luau,
            root: _,
            extends: _,
            include,
            exclude,
            entry,
            output_dir,
            output,
            entries,
            minify,
            dynamic_require,
            search_paths,
            transforms,
            profiles,
            preamble,
            luck_preamble,
            format,
            lint,
            overrides,
        } = config;
        if entry.is_some() && entries.is_some() {
            return Err(invalid("`entry` and `entries` can't both be set, so keep one"));
        }
        let root = config_directory(config_path);
        let filter =
            ProjectFilter::new(&root, include.as_deref(), exclude.as_deref().unwrap_or_default())
                .map_err(|message| invalid(&message))?;
        let compiled_overrides = overrides
            .unwrap_or_default()
            .into_iter()
            .map(|layer: OverrideConfig| {
                Ok(Override {
                    files: compiled(&layer.files)?,
                    exclude_files: compiled(&layer.exclude_files)?,
                    format: layer.format,
                    lint: layer.lint,
                })
            })
            .collect::<Result<_, String>>()
            .map_err(|message| invalid(&message))?;
        Ok(Self {
            root,
            files,
            lua: lua.unwrap_or(LuaTarget::Lua54),
            luau: luau.unwrap_or(LuaTarget::Luau),
            filter,
            format: format.unwrap_or_default(),
            lint: Arc::new(lint.unwrap_or_default()),
            overrides: compiled_overrides,
            search_paths: search_paths.unwrap_or_default(),
            dynamic_require: dynamic_require.unwrap_or_default(),
            transforms: transforms.unwrap_or_default(),
            build: BuildKeys {
                entry,
                output_dir,
                output,
                entries: entries.map(|Entries(list)| list),
                minify: minify.unwrap_or(false),
                profiles: profiles.unwrap_or_default(),
                preamble,
                luck_preamble: luck_preamble.unwrap_or(false),
            },
        })
    }

    /// The defaults, for files with no `luck.json` above them, rooted at
    /// `root`.
    #[must_use]
    pub fn defaults(root: PathBuf) -> Self {
        let filter = ProjectFilter::everything(&root);
        Self {
            root,
            files: Vec::new(),
            lua: LuaTarget::Lua54,
            luau: LuaTarget::Luau,
            filter,
            format: FormatConfig::default(),
            lint: Arc::default(),
            overrides: Vec::new(),
            search_paths: Vec::new(),
            dynamic_require: DynamicRequire::default(),
            transforms: TransformConfig::default(),
            build: BuildKeys::default(),
        }
    }

    /// The directory of the `luck.json`, which the project's relative paths
    /// and globs start from.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonical path of every config file the project read: its
    /// `luck.json` and each file that extends.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    #[must_use]
    pub const fn filter(&self) -> &ProjectFilter {
        &self.filter
    }

    /// The dialect of the file at `path`: the `luau` key's for a `.luau`
    /// file, and the `lua` key's for any other file or a document with no
    /// path.
    #[must_use]
    pub fn target_for(&self, path: Option<&Path>) -> LuaTarget {
        if path.and_then(Path::extension).is_some_and(|extension| extension == "luau") {
            self.luau
        } else {
            self.lua
        }
    }

    /// The `lint` section with the overrides `path` matches layered over it.
    #[must_use]
    pub fn lint_for(&self, path: Option<&Path>) -> Arc<LintConfig> {
        let overrides = self.overrides_for(path);
        let mut layers = overrides.iter().filter_map(|layer| layer.lint.as_ref()).peekable();
        if layers.peek().is_none() {
            return Arc::clone(&self.lint);
        }
        Arc::new(
            layers
                .fold(LintConfig::clone(&self.lint), |lint, layer| layer.clone().merge_onto(lint)),
        )
    }

    /// The defaults, then the `.editorconfig` files above `path` when there
    /// are `editorconfigs` to read them through, then the `format` section,
    /// then the overrides `path` matches.
    #[must_use]
    pub fn format_options(
        &self,
        path: Option<&Path>,
        editorconfigs: Option<&Editorconfigs>,
    ) -> FormatOptions {
        let format = self
            .overrides_for(path)
            .iter()
            .filter_map(|layer| layer.format)
            .fold(self.format, |format, layer| layer.merge_onto(format));
        let editorconfig = path
            .zip(editorconfigs)
            .map(|(file, reader)| reader.format_config_for(file))
            .unwrap_or_default();
        FormatOptions::from(&format.merge_onto(editorconfig))
    }

    /// Layers `overlay` over every file's lint settings, as command-line
    /// flags do.
    pub fn overlay_lint(&mut self, overlay: &LintConfig) {
        self.lint = Arc::new(overlay.clone().merge_onto(LintConfig::clone(&self.lint)));
        // Folded into each override too, it stays last however many a file
        // matches: merging is associative, and a layer repeated later
        // supersedes its earlier copy.
        for layer in self.overrides.iter_mut().filter_map(|layer| layer.lint.as_mut()) {
            *layer = overlay.clone().merge_onto(mem::take(layer));
        }
    }

    #[must_use]
    pub fn search_paths(&self) -> &[String] {
        &self.search_paths
    }

    #[must_use]
    pub const fn transforms(&self) -> TransformConfig {
        self.transforms
    }

    /// How a bundle for `version` resolves and reads its modules.
    #[must_use]
    pub fn bundle_options(&self, version: LuaVersion) -> BundleOptions<'_> {
        BundleOptions {
            version,
            search_paths: &self.search_paths,
            project_root: &self.root,
            dynamic_require: self.dynamic_require,
        }
    }

    /// The bundles `luck build` writes under `profile`.
    ///
    /// # Errors
    ///
    /// Returns a message when `profile` isn't defined, when there's no entry
    /// or output, when an entry isn't a file, or when an output collides with
    /// an entry or another output.
    pub fn builds(&self, profile: Option<&str>) -> Result<Vec<BuildConfig>, String> {
        self.build.builds(
            &self.root,
            self.transforms,
            |entry| self.target_for(Some(entry)),
            profile,
        )
    }

    fn overrides_for(&self, path: Option<&Path>) -> Vec<&Override> {
        let Some(file) = path.filter(|_| !self.overrides.is_empty()) else {
            return Vec::new();
        };
        let canonical = canonicalize_existing(file);
        let candidate = Candidate::new(self.filter.relative(&canonical));
        self.overrides
            .iter()
            .filter(|layer| {
                layer.files.is_match_candidate(&candidate)
                    && !layer.exclude_files.is_match_candidate(&candidate)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::project_with;

    #[test]
    fn a_file_no_override_matches_shares_the_base_lint_config() {
        let directory = project_with(&[(
            "luck.json",
            r#"{ "overrides": [{ "files": ["spec/**"], "lint": { "extra_globals": ["it"] } }] }"#,
        )]);
        let project = Project::load(&directory.path().join("luck.json")).expect("config loads");
        let base = project.lint_for(Some(&directory.path().join("src/a.lua")));
        assert!(Arc::ptr_eq(&base, &project.lint));
        let spec = project.lint_for(Some(&directory.path().join("spec/a.lua")));
        assert!(!Arc::ptr_eq(&spec, &project.lint));
    }
}
