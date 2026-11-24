use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{Candidate, GlobSet};
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_formatter::FormatOptions;
use luck_linter::{LintConfig, Linter};
use luck_minifier::TransformConfig;
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_stdlib::Stdlib;
use luck_token::{LuaTarget, LuaVersion};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::build::{BuildConfig, BuildKeys};
use crate::config::{Entries, LuckConfig, OverrideConfig};
use crate::editorconfig::Editorconfigs;
use crate::filter::ProjectFilter;
use crate::format_config::FormatConfig;
use crate::glob::compiled;
use crate::load::{self, Loaded, config_directory, in_file, read_std_definition};

/// A `luck.json` loaded through its `extends` chain and validated, or the
/// defaults where there's none, and the settings it gives each file.
#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    files: Vec<PathBuf>,
    /// The `lua` key's dialect's library, with the `std` definitions over it.
    lua: Stdlib,
    /// The `luau` key's dialect's library, with the `std` definitions over it.
    luau: Stdlib,
    filter: ProjectFilter,
    format: FormatConfig,
    overrides: Vec<Override>,
    /// The `lint` section compiled, for every file no override with `lint`
    /// settings matches.
    linter: Arc<Linter>,
    /// The linter of each set of overrides with `lint` settings, by their
    /// indices, compiled for the first file they all match.
    layered_linters: Mutex<FxHashMap<Vec<usize>, Arc<Linter>>>,
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
    /// Returns E010 when `config_path` or a std definition it names can't be
    /// read, and E014 when it, a file it extends, or a std definition isn't
    /// valid, each in the file at fault and placed where the problem is.
    pub fn load(config_path: &Path) -> Result<Self, FileDiagnostics> {
        let Loaded { config, mut files } = load::load(config_path)?;
        let invalid = |problem: &str| in_file(config_path, Diagnostic::e014(None, problem));
        let LuckConfig {
            lua,
            luau,
            std,
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
        let std_paths = std.unwrap_or_default();
        let definitions = std_paths
            .iter()
            .map(|std_path| read_std_definition(std_path))
            .collect::<Result<Vec<_>, _>>()?;
        let layered = |target: LuaTarget| {
            std_paths.iter().zip(&definitions).try_fold(
                Stdlib::new(target),
                |stdlib, (std_path, definition)| {
                    stdlib.layer(definition).map_err(|diag| in_file(std_path, diag))
                },
            )
        };
        let targets = (lua.unwrap_or(LuaTarget::Lua54), luau.unwrap_or(LuaTarget::Luau));
        let stdlibs = if targets.0 == targets.1 {
            let shared = layered(targets.0)?;
            (shared.clone(), shared)
        } else {
            (layered(targets.0)?, layered(targets.1)?)
        };
        files.extend(std_paths);
        Ok(Self {
            root,
            files,
            lua: stdlibs.0,
            luau: stdlibs.1,
            filter,
            format: format.unwrap_or_default(),
            overrides: compiled_overrides,
            linter: Arc::new(Linter::new(lint.unwrap_or_default())),
            layered_linters: Mutex::default(),
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
    pub fn defaults(root: PathBuf) -> Self {
        let filter = ProjectFilter::everything(&root);
        Self {
            root,
            files: Vec::new(),
            lua: Stdlib::new(LuaTarget::Lua54),
            luau: Stdlib::new(LuaTarget::Luau),
            filter,
            format: FormatConfig::default(),
            overrides: Vec::new(),
            linter: Arc::new(Linter::new(LintConfig::default())),
            layered_linters: Mutex::default(),
            search_paths: Vec::new(),
            dynamic_require: DynamicRequire::default(),
            transforms: TransformConfig::default(),
            build: BuildKeys::default(),
        }
    }

    /// The directory of the `luck.json`, which the project's relative paths
    /// and globs start from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonical path of every config file the project read: its
    /// `luck.json`, each file that extends, and each std definition.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub const fn filter(&self) -> &ProjectFilter {
        &self.filter
    }

    /// The dialect of the file at `path`: the `luau` key's for a `.luau`
    /// file, and the `lua` key's for any other file or a document with no
    /// path.
    pub fn target_for(&self, path: Option<&Path>) -> LuaTarget {
        self.stdlib_for(path).target()
    }

    /// The standard library of the file at `path`: its dialect's, with the
    /// `std` definitions layered over it.
    pub fn stdlib_for(&self, path: Option<&Path>) -> &Stdlib {
        if path.and_then(Path::extension).is_some_and(|extension| extension == "luau") {
            &self.luau
        } else {
            &self.lua
        }
    }

    /// The linter of the `lint` section with the overrides `path` matches
    /// layered over it, which every file those overrides match shares.
    pub fn linter_for(&self, path: Option<&Path>) -> Arc<Linter> {
        let (indices, layers): (Vec<usize>, Vec<&LintConfig>) = self
            .overrides_for(path)
            .into_iter()
            .filter_map(|(index, layer)| Some((index, layer.lint.as_ref()?)))
            .unzip();
        if indices.is_empty() {
            return Arc::clone(&self.linter);
        }
        Arc::clone(self.layered_linters.lock().entry(indices).or_insert_with(|| {
            let base = self.linter.config().clone();
            let lint = layers.into_iter().fold(base, |lint, layer| layer.clone().merge_onto(lint));
            Arc::new(Linter::new(lint))
        }))
    }

    /// The defaults, then the `.editorconfig` files above `path` when there
    /// are `editorconfigs` to read them through, then the `format` section,
    /// then the overrides `path` matches.
    pub fn format_options(
        &self,
        path: Option<&Path>,
        editorconfigs: Option<&Editorconfigs>,
    ) -> FormatOptions {
        let format = self
            .overrides_for(path)
            .into_iter()
            .filter_map(|(_, layer)| layer.format)
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
        let lint = overlay.clone().merge_onto(self.linter.config().clone());
        self.linter = Arc::new(Linter::new(lint));
        // Folded into each override too, it stays last however many a file
        // matches: merging is associative, and a layer repeated later
        // supersedes its earlier copy.
        for layer in self.overrides.iter_mut().filter_map(|layer| layer.lint.as_mut()) {
            *layer = overlay.clone().merge_onto(mem::take(layer));
        }
        self.layered_linters.get_mut().clear();
    }

    pub fn search_paths(&self) -> &[String] {
        &self.search_paths
    }

    pub const fn transforms(&self) -> TransformConfig {
        self.transforms
    }

    /// How a bundle for `version` resolves and reads its modules.
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

    /// Each override `path` matches, in order, with its index.
    fn overrides_for(&self, path: Option<&Path>) -> Vec<(usize, &Override)> {
        let Some(file) = path.filter(|_| !self.overrides.is_empty()) else {
            return Vec::new();
        };
        let canonical = FileSystemOs.canonicalize_existing(file);
        let candidate = Candidate::new(self.filter.relative(&canonical));
        self.overrides
            .iter()
            .enumerate()
            .filter(|(_, layer)| {
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
    fn files_that_match_the_same_overrides_share_one_linter() {
        let directory = project_with(&[(
            "luck.json",
            r#"{ "overrides": [
                { "files": ["spec/**"], "lint": { "extra_globals": ["it"] } },
                { "files": ["**/*.lua"], "format": { "line_width": 80 } }
            ] }"#,
        )]);
        let project = Project::load(&directory.path().join("luck.json")).expect("config loads");
        let linter_at = |relative: &str| project.linter_for(Some(&directory.path().join(relative)));
        assert!(Arc::ptr_eq(&linter_at("src/a.lua"), &project.linter));
        let spec = linter_at("spec/a.lua");
        assert!(!Arc::ptr_eq(&spec, &project.linter));
        assert!(Arc::ptr_eq(&spec, &linter_at("spec/b.lua")));
    }
}
