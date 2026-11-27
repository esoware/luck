use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{Candidate, GlobSet};
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_formatter::{FormatConfig, FormatOptions};
use luck_linter::{LintConfig, Linter};
use luck_minifier::TransformConfig;
use luck_resolver::FileSystem;
use luck_stdlib::SecurityLevel;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::build::{BuildConfig, BuildKeys};
use crate::config::{DefinitionsEntry, Entries, LuckConfig, OverrideConfig};
use crate::definitions::{DefinitionFiles, DefinitionGlob, Definitions};
use crate::editorconfig::Editorconfigs;
use crate::filter::{ProjectFilter, located_path};
use crate::glob::compiled;
use crate::load::{self, ConfigError, Loaded, config_directory, in_file, read_text};

/// A `luck.json` loaded through its `extends` chain and validated, a config
/// no file holds, or the defaults where there's none, and the settings it
/// gives each file, all read through one [`FileSystem`].
#[derive(Debug)]
pub struct Project {
    file_system: Arc<dyn FileSystem>,
    root: PathBuf,
    files: Vec<PathBuf>,
    /// The entries of `definitions`, resolved, which the overrides a file
    /// matches add theirs after.
    definition_entries: Vec<DefinitionsEntry>,
    /// The text of every definition file an entry takes, read once at load.
    definition_texts: FxHashMap<PathBuf, String>,
    security: Option<SecurityLevel>,
    /// What a file loads that no override with `definitions` or `security`
    /// matches.
    definitions: Arc<Definitions>,
    /// What the files load that each set of overrides with `definitions` or
    /// `security` matches, by their indices, built for the first file they
    /// all match.
    layered_definitions: Mutex<FxHashMap<Vec<usize>, Arc<Definitions>>>,
    filter: ProjectFilter,
    format: FormatConfig,
    overrides: Vec<Override>,
    /// The `lint` section compiled, for every file no override with `lint`
    /// settings matches.
    linter: Arc<Linter>,
    /// The linter of each set of overrides with `lint` settings, by their
    /// indices, compiled for the first file they all match.
    layered_linters: Mutex<FxHashMap<Vec<usize>, Arc<Linter>>>,
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
    definitions: Vec<DefinitionsEntry>,
    security: Option<SecurityLevel>,
}

impl Override {
    const fn layers_definitions(&self) -> bool {
        !self.definitions.is_empty() || self.security.is_some()
    }
}

impl Project {
    /// The project of the `luck.json` at `config_path` in `file_system`,
    /// rooted at its directory.
    ///
    /// # Errors
    ///
    /// Returns E010 when `config_path` or a definition file it takes can't
    /// be read, and E014 when it or a file it extends isn't valid, each in
    /// the file at fault and placed where the problem is. A definition file
    /// that doesn't parse is no error of the project, which leaves it out of
    /// [`Definitions::stdlib`].
    pub fn load(
        config_path: &Path,
        file_system: impl FileSystem + 'static,
    ) -> Result<Self, FileDiagnostics> {
        let file_system: Arc<dyn FileSystem> = Arc::new(file_system);
        let Loaded { config, files } = load::load(&*file_system, config_path)?;
        let invalid =
            |problem: &str| in_file(&*file_system, config_path, Diagnostic::e014(None, problem));
        let root = config_directory(config_path);
        Self::validated(config, files, root, Arc::clone(&file_system), invalid)
    }

    /// The project of `config`, the text of a config no file holds, such as
    /// one an editor gives, with the keys of a `luck.json` but `extends`,
    /// rooted at `root`, which its paths and globs are relative to, and
    /// reading through `file_system`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Given`], the E014 a `luck.json` with the same
    /// text would report, placed in `config`, or else the E010 of a
    /// definition file it names that can't be read, in that file.
    pub fn from_config(
        config: &str,
        root: PathBuf,
        file_system: impl FileSystem + 'static,
    ) -> Result<Self, ConfigError> {
        let file_system: Arc<dyn FileSystem> = Arc::new(file_system);
        let given = load::given(&*file_system, config, &root).map_err(ConfigError::Given)?;
        let invalid = |problem: &str| ConfigError::Given(Diagnostic::e014(None, problem));
        Self::validated(given, Vec::new(), root, file_system, invalid)
    }

    /// The project of `config`, which read `files`, rooted at `root`, with
    /// `invalid` placing a problem of the config as a whole.
    fn validated<E>(
        config: LuckConfig,
        mut files: Vec<PathBuf>,
        root: PathBuf,
        file_system: Arc<dyn FileSystem>,
        invalid: impl Fn(&str) -> E,
    ) -> Result<Self, E>
    where
        E: From<FileDiagnostics>,
    {
        let LuckConfig {
            security,
            definitions,
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
        let definition_entries = definitions.unwrap_or_default();
        let overrides = overrides.unwrap_or_default();
        let override_entries =
            overrides.iter().flat_map(|layer| layer.definitions.iter().flatten());
        let names_roblox = definition_entries
            .iter()
            .chain(override_entries)
            .any(|named| matches!(named, DefinitionsEntry::BuiltIn(_)));
        let sets_security =
            security.is_some() || overrides.iter().any(|layer| layer.security.is_some());
        if sets_security && !names_roblox {
            return Err(invalid(
                "`security` applies only to a project that loads `@roblox`, so add `@roblox` to \
                 `definitions` or remove `security`",
            ));
        }
        let filter = ProjectFilter::new(
            &root,
            include.as_deref(),
            exclude.as_deref().unwrap_or_default(),
            Arc::clone(&file_system),
        )
        .map_err(|message| invalid(&message))?;
        let compiled_overrides: Vec<Override> = overrides
            .into_iter()
            .map(|layer: OverrideConfig| {
                Ok(Override {
                    files: compiled(&layer.files)?,
                    exclude_files: compiled(&layer.exclude_files)?,
                    format: layer.format,
                    lint: layer.lint,
                    definitions: layer.definitions.unwrap_or_default(),
                    security: layer.security,
                })
            })
            .collect::<Result<_, String>>()
            .map_err(|message| invalid(&message))?;
        let mut definition_texts = FxHashMap::default();
        let every_entry = definition_entries
            .iter()
            .chain(compiled_overrides.iter().flat_map(|layer| &layer.definitions));
        for path in every_entry.flat_map(taken_paths) {
            if !definition_texts.contains_key(path) {
                let text = read_text(&*file_system, path)?;
                files.push(path.clone());
                definition_texts.insert(path.clone(), text);
            }
        }
        let base = Definitions::new(&definition_entries, security, &definition_texts);
        Ok(Self {
            file_system,
            root,
            files,
            definition_entries,
            definition_texts,
            security,
            definitions: Arc::new(base),
            layered_definitions: Mutex::default(),
            filter,
            format: format.unwrap_or_default(),
            overrides: compiled_overrides,
            linter: Arc::new(Linter::new(lint.unwrap_or_default())),
            layered_linters: Mutex::default(),
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
    /// `root`, reading through `file_system`.
    pub fn defaults(root: PathBuf, file_system: impl FileSystem + 'static) -> Self {
        let file_system: Arc<dyn FileSystem> = Arc::new(file_system);
        let filter = ProjectFilter::everything(&root, Arc::clone(&file_system));
        Self {
            file_system,
            root,
            files: Vec::new(),
            definition_entries: Vec::new(),
            definition_texts: FxHashMap::default(),
            security: None,
            definitions: Arc::default(),
            layered_definitions: Mutex::default(),
            filter,
            format: FormatConfig::default(),
            overrides: Vec::new(),
            linter: Arc::new(Linter::new(LintConfig::default())),
            layered_linters: Mutex::default(),
            dynamic_require: DynamicRequire::default(),
            transforms: TransformConfig::default(),
            build: BuildKeys::default(),
        }
    }

    /// The directory of the `luck.json`, or the root a config no file holds
    /// was given, which the project's relative paths and globs start from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonical path of every config file the project read: its
    /// `luck.json`, each file it extends, and each definition file.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub const fn filter(&self) -> &ProjectFilter {
        &self.filter
    }

    /// What a file loads that no override with `definitions` or `security`
    /// matches: what the top-level `definitions` loads, in order, at the
    /// top-level `security`.
    pub fn definitions(&self) -> &Definitions {
        &self.definitions
    }

    /// What the file at `path` loads: the top-level `definitions` with the
    /// `definitions` of each override it matches after them, at the
    /// `security` of the last of those that sets one, or else the top-level
    /// one. A document with no file loads [`Self::definitions`].
    pub fn definitions_for(&self, path: Option<&Path>) -> Arc<Definitions> {
        let (indices, layers): (Vec<usize>, Vec<&Override>) = self
            .overrides_for(path)
            .into_iter()
            .filter(|(_, layer)| layer.layers_definitions())
            .unzip();
        if indices.is_empty() {
            return Arc::clone(&self.definitions);
        }
        Arc::clone(self.layered_definitions.lock().entry(indices).or_insert_with(|| {
            let entries = self
                .definition_entries
                .iter()
                .chain(layers.iter().flat_map(|layer| &layer.definitions));
            let security = layers.iter().rev().find_map(|layer| layer.security).or(self.security);
            Arc::new(Definitions::new(entries, security, &self.definition_texts))
        }))
    }

    /// Whether `definitions`, the top-level one or an override's, names a
    /// definition file, a directory, or a glob, so a change to a definition
    /// file can change what the project's files load.
    pub fn reads_definition_files(&self) -> bool {
        self.taking_entries().next().is_some()
    }

    /// Whether a directory or glob in `definitions`, the top-level one or an
    /// override's, takes a definition file at `located`, a path
    /// [`located_path`] gives, which needn't exist, so creating or deleting
    /// one there changes what the project's files load.
    pub fn takes_definition_at(&self, located: &Path) -> bool {
        self.taking_entries()
            .filter_map(|files| match files {
                DefinitionFiles::Glob(glob) => Some(&**glob),
                DefinitionFiles::Written(_) | DefinitionFiles::File(_) => None,
            })
            .any(|glob: &DefinitionGlob| glob.takes(located))
    }

    /// The files each entry of `definitions` that takes files, rather than
    /// drops them, names, the top-level ones then each override's.
    fn taking_entries(&self) -> impl Iterator<Item = &DefinitionFiles> {
        self.definition_entries
            .iter()
            .chain(self.overrides.iter().flat_map(|layer| &layer.definitions))
            .filter_map(|entry| match entry {
                DefinitionsEntry::Files(files) => Some(files),
                DefinitionsEntry::BuiltIn(_) | DefinitionsEntry::Dropped(_) => None,
            })
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
            .map(|(file, reader)| reader.format_config_for(&*self.file_system, file))
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

    pub const fn transforms(&self) -> TransformConfig {
        self.transforms
    }

    /// How a bundle resolves and reads its modules, with no `.config.luau`
    /// reader, which a caller that links Luau gives.
    pub fn bundle_options(&self) -> BundleOptions<'_> {
        BundleOptions {
            project_root: &self.root,
            dynamic_require: self.dynamic_require,
            luau_config: None,
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
        self.build.builds(self, profile)
    }

    /// Each override `path` matches, in order, with its index.
    fn overrides_for(&self, path: Option<&Path>) -> Vec<(usize, &Override)> {
        let Some(file) = path.filter(|_| !self.overrides.is_empty()) else {
            return Vec::new();
        };
        let located = located_path(&*self.file_system, file);
        let candidate = Candidate::new(self.filter.relative(&located));
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

/// The definition files `entry` takes, rather than drops.
fn taken_paths(entry: &DefinitionsEntry) -> &[PathBuf] {
    match entry {
        DefinitionsEntry::Files(files) => files.paths(),
        DefinitionsEntry::BuiltIn(_) | DefinitionsEntry::Dropped(_) => &[],
    }
}

#[cfg(test)]
mod tests {
    use luck_resolver::FileSystemOs;

    use super::*;
    use crate::fixtures::project_with;

    #[test]
    fn files_that_match_the_same_overrides_share_one_set_of_definitions() {
        let directory = project_with(&[
            ("spec.d.luau", "declare it: (name: string) -> ()"),
            (
                "luck.json",
                r#"{ "overrides": [
                    { "files": ["spec/**"], "definitions": ["spec.d.luau"] },
                    { "files": ["**/*.luau"], "lint": { "extra_globals": ["a"] } }
                ] }"#,
            ),
        ]);
        let project =
            Project::load(&directory.path().join("luck.json"), FileSystemOs).expect("config loads");
        let at = |relative: &str| project.definitions_for(Some(&directory.path().join(relative)));
        assert!(Arc::ptr_eq(&at("src/a.luau"), &project.definitions));
        let spec = at("spec/a.luau");
        assert!(!Arc::ptr_eq(&spec, &project.definitions));
        assert!(Arc::ptr_eq(&spec, &at("spec/b.luau")));
        assert!(Arc::ptr_eq(&project.definitions_for(None), &project.definitions));
    }

    #[test]
    fn files_that_match_the_same_overrides_share_one_linter() {
        let directory = project_with(&[(
            "luck.json",
            r#"{ "overrides": [
                { "files": ["spec/**"], "lint": { "extra_globals": ["it"] } },
                { "files": ["**/*.luau"], "format": { "line_width": 80 } }
            ] }"#,
        )]);
        let project =
            Project::load(&directory.path().join("luck.json"), FileSystemOs).expect("config loads");
        let linter_at = |relative: &str| project.linter_for(Some(&directory.path().join(relative)));
        assert!(Arc::ptr_eq(&linter_at("src/a.luau"), &project.linter));
        let spec = linter_at("spec/a.luau");
        assert!(!Arc::ptr_eq(&spec, &project.linter));
        assert!(Arc::ptr_eq(&spec, &linter_at("spec/b.luau")));
    }
}
