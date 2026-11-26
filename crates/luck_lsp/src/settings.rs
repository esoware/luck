//! Discovery walks up from the document's directory, or once the client
//! gives a config, every document takes the project of that config.
//!
//! Results, misses and configs that don't load included, stay cached until a
//! config file changes, or the client gives another config. A lookup reads
//! the files, so only workers make one.

use std::ffi::OsString;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ls_types::Uri;
use luck_config::{
    CONFIG_FILE_NAME, ConfigError, EDITORCONFIG_FILE_NAME, Editorconfigs, Project, find_config,
};
use luck_formatter::FormatOptions;
use luck_linter::Linter;
use luck_resolver::{FileSystem as _, ResolverGeneric, is_definition_file};
use luck_type_checker::luau_config_aliases;
use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::files::Files;

/// What a document's path and the project around it fix, which every
/// version of the document shares until the settings change.
#[derive(Debug)]
pub(crate) struct DocumentSettings {
    /// Canonical, or `None` for a document with no file.
    pub path: Option<PathBuf>,
    pub project: Arc<LoadedProject>,
    /// The linter of the project's lint settings, with the overrides the
    /// path matches. Publishing, code actions, and fix-all all lint with it,
    /// so none of them fixes a diagnostic the user was never shown.
    pub linter: Arc<Linter>,
    pub format: FormatOptions,
    /// Whether the project's `include` and `exclude` take the document in.
    /// One they leave out gets no diagnostics and no fixes.
    pub is_included: bool,
    /// Whether it's a definition file, by its name, which parses as one and
    /// is no module.
    pub is_definitions: bool,
}

/// A project, and the resolver its files' requires go through, so each
/// request answers from what the ones before it read.
#[derive(Debug)]
pub(crate) struct LoadedProject {
    project: Project,
    files: Files,
    resolver: ResolverGeneric<Files>,
}

impl LoadedProject {
    pub(crate) fn new(project: Project, files: Files) -> Self {
        let resolver = ResolverGeneric::new_with_file_system(files.clone())
            .with_luau_config(luau_config_aliases);
        Self { project, files, resolver }
    }

    pub(crate) const fn project(&self) -> &Project {
        &self.project
    }

    pub(crate) const fn resolver(&self) -> &ResolverGeneric<Files> {
        &self.resolver
    }

    /// The URI of the file at `path`, as the project's files name it.
    pub(crate) fn uri(&self, path: &Path) -> Option<Uri> {
        self.files.uri(path)
    }
}

/// A config that doesn't load, which the first document under it reports.
#[derive(Debug)]
pub(crate) struct LoadError {
    pub error: ConfigError,
    /// The cache's [`SettingsCache::generation`] when the lookup read it.
    pub generation: u64,
}

/// The config a client gives, which every document's project comes from in
/// place of a `luck.json`.
#[derive(Debug)]
pub(crate) struct GivenConfig {
    /// The directory its paths and globs are relative to.
    pub root: PathBuf,
    /// As JSON, which the client replaces with each change it sends.
    pub text: RwLock<String>,
}

#[derive(Debug)]
pub(crate) struct SettingsCache {
    files: Files,
    given: Option<GivenConfig>,
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
    /// By the directory a document is in. A client's config gives every
    /// document the project the untitled ones get.
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
    /// JSON file a broken chain might extend, and any Luau file it might name
    /// as a definition file, matters.
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
        error: Option<ConfigError>,
    ) -> (Arc<LoadedProject>, Option<ConfigError>) {
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
        if let Some(ConfigError::File(broken)) = &error {
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
    /// Reads every project through `files`, and gives every document the
    /// project of `given`, when the client gives a config.
    pub(crate) fn new(files: Files, given: Option<GivenConfig>) -> Self {
        Self { files, given, lookups: RwLock::default() }
    }

    /// Whether the client gave a config.
    pub(crate) const fn is_given(&self) -> bool {
        self.given.is_some()
    }

    /// Makes `text`, which the client sent, the config it gave, which the
    /// next lookup after a [`clear`](Self::clear) loads.
    pub(crate) fn set_given(&self, text: String) {
        if let Some(given) = &self.given {
            *given.text.write() = text;
        }
    }

    /// The settings of the document at `file`, a path
    /// [`located_path`](luck_config::located_path) gives, or of an untitled
    /// document, under `project`, the one [`Self::project_for`] gives it.
    pub(crate) fn document_settings(
        &self,
        project: Arc<LoadedProject>,
        file: Option<&Path>,
    ) -> DocumentSettings {
        let editorconfigs = Arc::clone(&self.lookups.read().editorconfigs);
        let loaded = project.project();
        let linter = loaded.linter_for(file);
        let format = loaded.format_options(file, Some(&editorconfigs));
        let is_included = file.is_none_or(|located| loaded.filter().includes_document(located));
        let is_definitions = file.is_some_and(is_definition_file);
        let path = file.map(|located| self.files.canonicalize_existing(located));
        DocumentSettings { path, project, linter, format, is_included, is_definitions }
    }

    /// The project of the nearest `luck.json` above the file at
    /// `document_path`, whose directory is canonical, so each directory and
    /// config has one spelling, or the defaults rooted at its directory when
    /// there's none, and why that `luck.json` doesn't load, if this is the
    /// first lookup to find it. A `luck.json` that doesn't load gives the
    /// defaults rooted at its directory. With a config the client gave, it's
    /// that config's project for every document, or the defaults rooted where
    /// it is.
    ///
    /// `spelled` is the document's path as the client spells it. A load error
    /// names its file under the same directories, since a client knows a file
    /// only by its own spelling.
    pub(crate) fn project_for(
        &self,
        document_path: Option<&Path>,
        spelled: Option<&Path>,
    ) -> (Arc<LoadedProject>, Option<LoadError>) {
        let directory = document_path.filter(|_| self.given.is_none()).and_then(Path::parent);
        let generation = {
            let lookups = self.lookups.read();
            if let Some(project) = lookups.cached(directory) {
                return (Arc::clone(project), None);
            }
            lookups.generation
        };
        let files = &self.files;
        let loaded = |project| Arc::new(LoadedProject::new(project, files.clone()));
        let defaults = |root| loaded(Project::defaults(root, files.clone()));
        if let Some(given) = &self.given {
            let text = given.text.read().clone();
            let (project, error) =
                match Project::from_config(&text, given.root.clone(), files.clone()) {
                    Ok(project) => (loaded(project), None),
                    Err(error) => (defaults(given.root.clone()), Some(error)),
                };
            return self.keep(generation, None, None, project, error);
        }
        // Discovery stats every directory up to the root, loading reads every
        // file in the `extends` chain, and the defaults' filter canonicalizes
        // their root.
        let config_path = directory.and_then(|start| find_config(files, start));
        let shared = config_path
            .as_ref()
            .and_then(|config| self.lookups.read().by_config.get(config).cloned());
        let (project, error) = match (shared, &config_path) {
            (Some(project), _) => (project, None),
            (None, Some(config)) => match Project::load(config, files.clone()) {
                Ok(project) => (loaded(project), None),
                Err(mut problem) => {
                    if let Some(document) = spelled {
                        problem.path = spelled_beside(files, &problem.path, document);
                    }
                    let root = config.parent().map_or_else(PathBuf::new, Path::to_path_buf);
                    (defaults(root), Some(ConfigError::File(problem)))
                }
            },
            (None, None) => {
                let root = directory.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
                (defaults(root), None)
            }
        };
        self.keep(generation, directory, config_path, project, error)
    }

    /// Keeps `project`, which a lookup in `generation` found, unless the cache
    /// was cleared since.
    fn keep(
        &self,
        generation: u64,
        directory: Option<&Path>,
        config_path: Option<PathBuf>,
        project: Arc<LoadedProject>,
        error: Option<ConfigError>,
    ) -> (Arc<LoadedProject>, Option<LoadError>) {
        let mut lookups = self.lookups.write();
        if lookups.generation != generation {
            return (project, None);
        }
        let (kept, reported) = lookups.keep(directory, config_path, project, error);
        (kept, reported.map(|problem| LoadError { error: problem, generation }))
    }

    /// Bumped by each [`clear`](Self::clear).
    pub(crate) fn generation(&self) -> u64 {
        self.lookups.read().generation
    }

    /// Whether a change to the file at `path` can change a document's
    /// settings. With a config the client gave, any definition file can,
    /// since the config may name one the client hasn't pushed yet.
    pub(crate) fn is_settings_file(&self, path: &Path) -> bool {
        let Some(name) = path.file_name() else {
            return false;
        };
        let lookups = self.lookups.read();
        (self.given.is_some() && is_definition_file(path))
            || name == CONFIG_FILE_NAME
            || name == EDITORCONFIG_FILE_NAME
            || lookups.config_names.contains(name)
            || (!lookups.broken.is_empty()
                && path.extension().is_some_and(|extension| {
                    ["json", "luau", "lua"].iter().any(|known| extension == *known)
                }))
    }

    /// Forgets every lookup, every `.editorconfig` read, and every resolver,
    /// and returns each config file a load error was reported in.
    pub(crate) fn clear(&self) -> Vec<PathBuf> {
        let mut lookups = self.lookups.write();
        let generation = lookups.generation + 1;
        let Lookups { broken, .. } =
            mem::replace(&mut *lookups, Lookups { generation, ..Lookups::default() });
        broken
    }

    /// Makes every project's resolver read the disk again, as a module
    /// created or deleted, or a `.luaurc` edited, needs.
    pub(crate) fn clear_resolvers(&self) {
        for project in &self.lookups.read().projects {
            project.resolver.clear_cache();
        }
    }
}

/// `path`, a canonical path in `files`, as the client spells the files near
/// `document`.
///
/// That's under the deepest directory above `document` whose canonical path
/// holds it, so a client that reached `document` through a symlink or a
/// short name knows the file, or `path` itself when none holds it.
fn spelled_beside(files: &Files, path: &Path, document: &Path) -> PathBuf {
    document
        .ancestors()
        .skip(1)
        .find_map(|directory| {
            let inside = path.strip_prefix(files.canonicalize_existing(directory)).ok()?;
            Some(directory.join(inside))
        })
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::ptr;

    use luck_resolver::FileSystemOs;

    use super::*;

    #[test]
    fn a_document_with_no_config_takes_the_defaults_rooted_at_its_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("foo.luau");
        let cache = SettingsCache::new(Files::Disk, None);
        let (project, error) = cache.project_for(Some(&file), None);
        assert!(error.is_none());
        assert!(!project.project().stdlib().library().has_roblox);
        if find_config(&FileSystemOs, directory.path()).is_none() {
            assert_eq!(project.project().root(), directory.path());
        }
        let (untitled, _) = cache.project_for(None, None);
        assert!(!untitled.project().stdlib().library().has_roblox);
        assert!(Arc::ptr_eq(&untitled, &cache.project_for(None, None).0));
    }

    #[test]
    fn cache_keeps_projects_until_cleared() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, r#"{ "definitions": ["@roblox"] }"#).expect("write luck.json");
        fs::create_dir_all(directory.path().join("sub")).expect("mkdir sub");
        let first = directory.path().join("first.luau");
        let nested = directory.path().join("sub/nested.luau");
        let cache = SettingsCache::new(Files::Disk, None);

        let (project, _) = cache.project_for(Some(&first), None);
        assert!(project.project().stdlib().library().has_roblox);
        assert!(Arc::ptr_eq(&project, &cache.project_for(Some(&nested), None).0));

        fs::write(&config_path, "{}").expect("rewrite luck.json");
        let (cached, _) = cache.project_for(Some(&first), None);
        assert!(cached.project().stdlib().library().has_roblox);
        let generation = cache.generation();
        cache.clear();
        assert_ne!(cache.generation(), generation, "a clear starts a new generation");
        let (reloaded, _) = cache.project_for(Some(&first), None);
        assert!(!reloaded.project().stdlib().library().has_roblox);
    }

    #[test]
    fn a_broken_config_is_reported_once_and_its_files_take_the_defaults() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), r#"{ "definitions": ["@bogus"] }"#)
            .expect("write luck.json");
        let file = directory.path().join("a.luau");
        let cache = SettingsCache::new(Files::Disk, None);

        let (project, first_error) = cache.project_for(Some(&file), None);
        let error = first_error.expect("the error comes with the first lookup");
        let ConfigError::File(problem) = &error.error else { panic!("{error:?}") };
        assert_eq!(problem.diags[0].code, "E014");
        assert_eq!(error.generation, cache.generation());
        assert!(!project.project().stdlib().library().has_roblox);
        assert!(cache.project_for(Some(&file), None).1.is_none(), "reported once");

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
        let cache = SettingsCache::new(Files::Disk, None);
        let sourcemap = directory.path().join("sourcemap.json");
        let shared = directory.path().join("shared.json");
        assert!(!cache.is_settings_file(&shared));

        let _ = cache.project_for(Some(&directory.path().join("a.luau")), None);
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
        let main = directory.path().join("main.luau");
        let cache = SettingsCache::new(Files::Disk, None);
        let (project, _) = cache.project_for(Some(&main), None);
        let resolver = project.resolver();
        assert!(ptr::eq(resolver, cache.project_for(Some(&main), None).0.resolver()));
        assert!(resolver.resolve(&main, "./late").is_err(), "nothing named `late` exists yet");

        fs::write(directory.path().join("late.luau"), "return {}\n").expect("write late.luau");
        assert!(resolver.resolve(&main, "./late").is_err(), "the listing is cached");
        cache.clear_resolvers();
        assert!(resolver.resolve(&main, "./late").is_ok(), "the cleared resolver reads the disk");
    }
}
