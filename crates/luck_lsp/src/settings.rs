//! Discovery walks up from the document's directory.
//!
//! Results, misses and configs that don't load included, stay cached until a
//! watched config file changes. A lookup reads the disk, so only workers make
//! one.

use alloc::sync::Arc;
use core::mem;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use luck_config::{CONFIG_FILE_NAME, Editorconfigs, Project, find_config};
use luck_diagnostics::FileDiagnostics;
use luck_formatter::FormatOptions;
use luck_linter::LintConfig;
use luck_resolver::{ResolveOptions, Resolver};
use luck_stdlib::Stdlib;
use luck_token::LuaVersion;
use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHashSet};

/// The other file whose change can change a document's settings.
const EDITORCONFIG_FILE_NAME: &str = ".editorconfig";

/// What a document's path and the project around it fix, which every
/// version of the document shares until the settings change.
#[derive(Debug)]
pub(crate) struct DocumentSettings {
    pub project: Arc<LoadedProject>,
    /// The standard library of the document's dialect, with the project's
    /// std definitions over it.
    pub stdlib: Stdlib,
    /// The project's lint settings, with the overrides the path matches.
    /// Publishing, code actions, and fix-all all lint with these, so none of
    /// them fixes a diagnostic the user was never shown.
    pub lint: Arc<LintConfig>,
    pub format: FormatOptions,
    /// Whether the project's `include` and `exclude` take the document in.
    /// One they leave out gets no diagnostics and no fixes.
    pub is_included: bool,
}

/// A project, and the resolvers its files' requires go through, so each
/// request answers from what the ones before it read.
#[derive(Debug)]
pub(crate) struct LoadedProject {
    project: Project,
    lua: OnceLock<Resolver>,
    luau: OnceLock<Resolver>,
}

impl LoadedProject {
    pub(crate) const fn new(project: Project) -> Self {
        Self { project, lua: OnceLock::new(), luau: OnceLock::new() }
    }

    pub(crate) const fn project(&self) -> &Project {
        &self.project
    }

    /// The resolver for a file of `version`. Every Lua version searches the
    /// same templates, so they share one.
    pub(crate) fn resolver(&self, version: LuaVersion) -> &Resolver {
        let resolver = if version.is_luau() { &self.luau } else { &self.lua };
        resolver.get_or_init(|| {
            Resolver::new(ResolveOptions {
                version,
                search_paths: self.project.search_paths().to_vec(),
                root: self.project.root().to_path_buf(),
            })
        })
    }

    fn clear_resolvers(&self) {
        for resolver in [&self.lua, &self.luau].into_iter().filter_map(OnceLock::get) {
            resolver.clear_cache();
        }
    }
}

/// A `luck.json` that doesn't load, which the first document under it
/// reports.
#[derive(Debug)]
pub(crate) struct ConfigError {
    pub file: FileDiagnostics,
    /// The cache's [`SettingsCache::generation`] when the lookup read it.
    pub generation: u64,
}

#[derive(Debug, Default)]
pub(crate) struct SettingsCache {
    /// One lock for all of them, so a reader never sees a directory without
    /// its project.
    lookups: RwLock<Lookups>,
}

#[derive(Debug, Default)]
struct Lookups {
    /// Bumped by each clear, so a lookup that read the disk before one
    /// neither keeps nor reports what it read.
    generation: u64,
    /// Shared by every document without a file path.
    untitled: Option<Arc<LoadedProject>>,
    /// By the directory a document is in.
    by_directory: FxHashMap<PathBuf, Arc<LoadedProject>>,
    /// By the path of their `luck.json`, so documents in several of a
    /// project's directories share it.
    by_config: FxHashMap<PathBuf, Arc<LoadedProject>>,
    /// Every project above, once each.
    projects: Vec<Arc<LoadedProject>>,
    /// The `.editorconfig` files formatting has read, each read once until a
    /// watched config file changes.
    editorconfigs: Arc<Editorconfigs>,
    /// The name of every file a project read, beside `luck.json`, which a
    /// watched change has to touch to matter.
    config_names: FxHashSet<OsString>,
    /// Each config file a reported load error is in. While there's one, any
    /// JSON file a broken chain might extend, and any TOML file it might name
    /// as a std definition, matters.
    broken: Vec<PathBuf>,
}

impl Lookups {
    fn cached(&self, directory: Option<&Path>) -> Option<&Arc<LoadedProject>> {
        directory.map_or(self.untitled.as_ref(), |known| self.by_directory.get(known))
    }

    /// Keeps `project` for documents in `directory`, unless another document
    /// found the same project meanwhile and kept it, and reported its error.
    /// The error comes back only with a project kept for the first time.
    fn keep(
        &mut self,
        directory: Option<&Path>,
        config_path: Option<PathBuf>,
        project: Arc<LoadedProject>,
        error: Option<FileDiagnostics>,
    ) -> (Arc<LoadedProject>, Option<FileDiagnostics>) {
        let found_meanwhile = match &config_path {
            Some(config) => self.by_config.get(config),
            None => self.cached(directory),
        };
        if let Some(existing) = found_meanwhile.cloned() {
            self.remember(directory, &existing);
            return (existing, None);
        }
        let names = project.project().files().iter().filter_map(|file| file.file_name());
        self.config_names.extend(names.map(OsString::from));
        if let Some(broken) = &error {
            self.config_names.extend(broken.path.file_name().map(OsString::from));
            self.broken.push(broken.path.clone());
        }
        if let Some(config) = config_path {
            self.by_config.insert(config, Arc::clone(&project));
        }
        self.projects.push(Arc::clone(&project));
        self.remember(directory, &project);
        (project, error)
    }

    /// Makes `project` the one documents in `directory` get.
    fn remember(&mut self, directory: Option<&Path>, project: &Arc<LoadedProject>) {
        match directory {
            Some(found) => {
                self.by_directory.insert(found.to_path_buf(), Arc::clone(project));
            }
            None => self.untitled = Some(Arc::clone(project)),
        }
    }
}

impl SettingsCache {
    /// The settings of the document at `path`, or of an untitled document,
    /// and why its `luck.json` doesn't load, if this is the first document
    /// to ask.
    pub(crate) fn document_settings(
        &self,
        path: Option<&Path>,
    ) -> (DocumentSettings, Option<ConfigError>) {
        let (project, error) = self.project_for(path);
        let editorconfigs = Arc::clone(&self.lookups.read().editorconfigs);
        let loaded = project.project();
        let settings = DocumentSettings {
            project: Arc::clone(&project),
            stdlib: loaded.stdlib_for(path).clone(),
            lint: loaded.lint_for(path),
            format: loaded.format_options(path, Some(&editorconfigs)),
            is_included: path.is_none_or(|file| loaded.filter().is_included(file)),
        };
        (settings, error)
    }

    /// The project of the nearest `luck.json` above the document at
    /// `document_path`, or the defaults rooted at its directory when there's
    /// none. A `luck.json` that doesn't load gives the defaults rooted at its
    /// directory, with the error.
    fn project_for(
        &self,
        document_path: Option<&Path>,
    ) -> (Arc<LoadedProject>, Option<ConfigError>) {
        let directory = document_path.and_then(Path::parent);
        let generation = {
            let lookups = self.lookups.read();
            if let Some(project) = lookups.cached(directory) {
                return (Arc::clone(project), None);
            }
            lookups.generation
        };
        // Discovery stats every directory up to the root, loading reads every
        // file in the `extends` chain, and the defaults' filter canonicalizes
        // their root.
        let config_path = directory.and_then(find_config);
        let shared = config_path
            .as_ref()
            .and_then(|config| self.lookups.read().by_config.get(config).cloned());
        let (project, error) = match (shared, &config_path) {
            (Some(project), _) => (project, None),
            (None, Some(config)) => match Project::load(config) {
                Ok(loaded) => (Arc::new(LoadedProject::new(loaded)), None),
                Err(problem) => {
                    let root = config.parent().map_or_else(PathBuf::new, Path::to_path_buf);
                    (Arc::new(LoadedProject::new(Project::defaults(root))), Some(problem))
                }
            },
            (None, None) => {
                let root = directory.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
                (Arc::new(LoadedProject::new(Project::defaults(root))), None)
            }
        };
        let mut lookups = self.lookups.write();
        if lookups.generation != generation {
            return (project, None);
        }
        let (kept, reported) = lookups.keep(directory, config_path, project, error);
        drop(lookups);
        (kept, reported.map(|file| ConfigError { file, generation }))
    }

    /// Bumped by each [`clear`](Self::clear).
    pub(crate) fn generation(&self) -> u64 {
        self.lookups.read().generation
    }

    /// Whether a change to the file at `path` can change a document's
    /// settings.
    pub(crate) fn is_settings_file(&self, path: &Path) -> bool {
        let Some(name) = path.file_name() else {
            return false;
        };
        let lookups = self.lookups.read();
        name == CONFIG_FILE_NAME
            || name == EDITORCONFIG_FILE_NAME
            || lookups.config_names.contains(name)
            || (!lookups.broken.is_empty()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json" || extension == "toml"))
    }

    /// Forgets every lookup, every `.editorconfig` read, and every resolver,
    /// and returns each config file a load error was reported in.
    pub(crate) fn clear(&self) -> Vec<PathBuf> {
        let mut lookups = self.lookups.write();
        let generation = lookups.generation.wrapping_add(1);
        let Lookups { broken, .. } =
            mem::replace(&mut *lookups, Lookups { generation, ..Lookups::default() });
        broken
    }

    /// Makes every project's resolvers read the disk again, as a module
    /// created or deleted, or a `.luaurc` edited, needs.
    pub(crate) fn clear_resolvers(&self) {
        for project in &self.lookups.read().projects {
            project.clear_resolvers();
        }
    }
}

#[cfg(test)]
mod tests {
    use core::ptr;
    use std::fs;

    use luck_token::LuaTarget;

    use super::*;

    #[test]
    fn a_document_with_no_config_takes_the_defaults_rooted_at_its_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("foo.lua");
        let cache = SettingsCache::default();
        let (project, error) = cache.project_for(Some(&file));
        assert!(error.is_none());
        assert_eq!(project.project().target_for(Some(&file)), LuaTarget::Lua54);
        if find_config(directory.path()).is_none() {
            assert_eq!(project.project().root(), directory.path());
        }
        let (untitled, _) = cache.project_for(None);
        assert_eq!(untitled.project().target_for(None), LuaTarget::Lua54);
        assert!(Arc::ptr_eq(&untitled, &cache.project_for(None).0));
    }

    #[test]
    fn cache_keeps_projects_until_cleared() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, r#"{ "lua": "lua53" }"#).expect("write luck.json");
        fs::create_dir_all(directory.path().join("sub")).expect("mkdir sub");
        let first = directory.path().join("first.lua");
        let nested = directory.path().join("sub/nested.lua");
        let cache = SettingsCache::default();

        let (project, _) = cache.project_for(Some(&first));
        assert_eq!(project.project().target_for(Some(&first)), LuaTarget::Lua53);
        assert!(Arc::ptr_eq(&project, &cache.project_for(Some(&nested)).0));

        fs::write(&config_path, r#"{ "lua": "lua51" }"#).expect("rewrite luck.json");
        let (cached, _) = cache.project_for(Some(&first));
        assert_eq!(cached.project().target_for(Some(&first)), LuaTarget::Lua53);
        let generation = cache.generation();
        cache.clear();
        assert_ne!(cache.generation(), generation, "a clear starts a new generation");
        let (reloaded, _) = cache.project_for(Some(&first));
        assert_eq!(reloaded.project().target_for(Some(&first)), LuaTarget::Lua51);
    }

    #[test]
    fn a_broken_config_is_reported_once_and_its_files_take_the_defaults() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), r#"{ "lua": "lua99" }"#)
            .expect("write luck.json");
        let file = directory.path().join("a.lua");
        let cache = SettingsCache::default();

        let (project, first_error) = cache.project_for(Some(&file));
        let error = first_error.expect("the error comes with the first lookup");
        assert_eq!(error.file.diags[0].code, "E014");
        assert_eq!(error.generation, cache.generation());
        assert_eq!(project.project().target_for(Some(&file)), LuaTarget::Lua54);
        assert!(cache.project_for(Some(&file)).1.is_none(), "reported once");

        assert!(cache.is_settings_file(&directory.path().join("other.json")));
        let broken = cache.clear();
        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(!cache.is_settings_file(&directory.path().join("other.json")));
    }

    #[test]
    fn a_file_a_project_extends_is_a_settings_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("shared.json"), "{}").expect("write shared.json");
        fs::write(directory.path().join("luck.json"), r#"{ "extends": ["shared.json"] }"#)
            .expect("write luck.json");
        let cache = SettingsCache::default();
        let sourcemap = directory.path().join("sourcemap.json");
        let shared = directory.path().join("shared.json");
        assert!(!cache.is_settings_file(&shared));

        let _discovered = cache.project_for(Some(&directory.path().join("a.lua")));
        assert!(cache.is_settings_file(&shared));
        assert!(cache.is_settings_file(&directory.path().join("luck.json")));
        assert!(cache.is_settings_file(&directory.path().join(".editorconfig")));
        assert!(!cache.is_settings_file(&sourcemap), "Rojo rewrites it on every change");
        assert!(
            !cache.is_settings_file(&directory.path().join(".luaurc")),
            "a `.luaurc` changes how requires resolve, not settings"
        );
    }

    #[test]
    fn a_project_resolves_through_one_resolver_until_its_cache_is_cleared() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), "{}").expect("write luck.json");
        let main = directory.path().join("main.lua");
        let cache = SettingsCache::default();
        let (project, _) = cache.project_for(Some(&main));
        let resolver = project.resolver(LuaVersion::Lua54);
        assert!(ptr::eq(resolver, project.resolver(LuaVersion::Lua51)));
        assert!(resolver.resolve(&main, "late").is_err(), "nothing named `late` exists yet");

        fs::write(directory.path().join("late.lua"), "return {}\n").expect("write late.lua");
        assert!(resolver.resolve(&main, "late").is_err(), "the listing is cached");
        cache.clear_resolvers();
        assert!(resolver.resolve(&main, "late").is_ok(), "the cleared resolver reads the disk");
    }
}
