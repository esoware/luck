//! Discovery walks up from the document's directory, or once the client
//! gives a config, every document takes the project of that config.
//!
//! Results, misses and configs that don't load included, stay cached until a
//! config file changes, or the client gives another config, and a project
//! that reads a definition file until that file changes, or one with a
//! directory or glob in `definitions` until a definition file it takes is
//! created or deleted. A lookup reads the files, so only workers make one.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ls_types::Uri;
use luck_config::{
    CONFIG_FILE_NAME, ConfigError, Definitions, Editorconfigs, Project, find_config,
};
use luck_linter::Linter;
use luck_resolver::{FileSystem as _, ResolverGeneric, is_definition_file};
use parking_lot::{Mutex, RwLock};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::files::Files;
use crate::type_check::{TypeCheck, TypeChecks};

/// What a document's path and the project around it fix, which every
/// version of the document shares until the settings change.
#[derive(Debug)]
pub(crate) struct DocumentSettings {
    /// Canonical, or `None` for a document with no file.
    pub path: Option<PathBuf>,
    /// As [`located_path`](luck_config::located_path) gives it, which the
    /// project's globs and the `.editorconfig` files match.
    pub located: Option<PathBuf>,
    pub project: Arc<LoadedProject>,
    /// The linter of the project's lint settings, with the overrides the
    /// path matches. Publishing, code actions, and fix-all all lint with it,
    /// so none of them fixes a diagnostic the user was never shown.
    pub linter: Arc<Linter>,
    /// Whether the project's `include` and `exclude` take the document in.
    /// One they leave out gets no diagnostics and no fixes.
    pub is_included: bool,
    /// Whether it's a definition file, by its name, which parses as one and
    /// is no module.
    pub is_definitions: bool,
    /// What it loads: the project's definitions, with those of the overrides
    /// its path matches.
    pub definitions: Arc<Definitions>,
}

/// A project, with the session's resolver, which its files' requires go
/// through, and the checker of each set of definitions its files load once
/// a job needs it.
#[derive(Debug)]
pub(crate) struct LoadedProject {
    project: Project,
    files: Files,
    resolver: Arc<ResolverGeneric<Files>>,
    checks: Mutex<Vec<(Arc<Definitions>, Arc<TypeCheck>)>>,
}

impl LoadedProject {
    pub(crate) fn new(
        project: Project,
        files: Files,
        resolver: Arc<ResolverGeneric<Files>>,
    ) -> Self {
        Self { project, files, resolver, checks: Mutex::default() }
    }

    pub(crate) const fn project(&self) -> &Project {
        &self.project
    }

    pub(crate) fn resolver(&self) -> &ResolverGeneric<Files> {
        &self.resolver
    }

    /// The URI of the file at `path`, as the project's files name it.
    pub(crate) fn uri(&self, path: &Path) -> Option<Uri> {
        self.files.uri(path)
    }

    /// The checker of its files that load `definitions`, one of its
    /// project's, which it holds from the first time a job asks, so `checks`
    /// keeps it.
    pub(crate) fn type_check(
        &self,
        checks: &TypeChecks,
        definitions: &Arc<Definitions>,
    ) -> Arc<TypeCheck> {
        let mut held = self.checks.lock();
        if let Some((_, check)) = held.iter().find(|(loaded, _)| Arc::ptr_eq(loaded, definitions)) {
            return Arc::clone(check);
        }
        let check = checks.of(definitions);
        held.push((Arc::clone(definitions), Arc::clone(&check)));
        check
    }

    /// Whether a change to the definition file at `located`, a path
    /// [`located_path`](luck_config::located_path) gives, can change what its
    /// files load: it reads a definition file of that name, or a directory or
    /// glob in its `definitions` takes one there.
    pub(crate) fn reads_definition(&self, located: &Path) -> bool {
        let name = located.file_name();
        let files = self.project.files();
        files.iter().any(|file| is_definition_file(file) && file.file_name() == name)
            || self.project.takes_definition_at(located)
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
    resolver: Arc<ResolverGeneric<Files>>,
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
    /// Bumped by each change to a definition file, so a lookup that read
    /// one before keeps no project that lists one.
    definition_edits: u64,
    /// Shared by every document without a file path.
    untitled: Option<Arc<LoadedProject>>,
    /// By the directory a document is in. A client's config gives every
    /// document the project the untitled ones get.
    by_directory: FxHashMap<PathBuf, Arc<LoadedProject>>,
    /// By the path of their `luck.json`, so documents in several of a
    /// project's directories share it.
    by_config: FxHashMap<PathBuf, Arc<LoadedProject>>,
    /// The `.editorconfig` files formatting has read, each read once until a
    /// watched one changes.
    editorconfigs: Arc<Editorconfigs>,
    /// The name of every file but a definition file that a project read
    /// beside `luck.json`, which a watched change has to touch to matter.
    config_names: FxHashSet<OsString>,
    /// Whether a lookup reported a config that doesn't load. While one
    /// doesn't, creating any JSON file a broken chain might extend, or any
    /// Luau file it might name as a definition file, matters.
    is_broken: bool,
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
        let read = project.project().files().iter().filter(|file| !is_definition_file(file));
        self.config_names.extend(read.filter_map(|file| file.file_name()).map(OsString::from));
        if let Some(problem) = &error {
            self.is_broken = true;
            if let ConfigError::File(broken) = problem {
                self.config_names.extend(broken.path.file_name().map(OsString::from));
            }
        }
        if let Some(config) = config_path {
            self.by_config.insert(config, Arc::clone(&project));
        }
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
    /// Reads every project through `files`, resolving their requires through
    /// `resolver`, and gives every document the project of `given`, when the
    /// client gives a config.
    pub(crate) fn new(
        files: Files,
        resolver: Arc<ResolverGeneric<Files>>,
        given: Option<GivenConfig>,
    ) -> Self {
        Self { files, resolver, given, lookups: RwLock::default() }
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
        let loaded = project.project();
        let linter = loaded.linter_for(file);
        let is_included = file.is_none_or(|located| loaded.filter().includes_document(located));
        let is_definitions = file.is_some_and(is_definition_file);
        let definitions = loaded.definitions_for(file);
        let path = file.map(|located| self.files.canonicalize_existing(located));
        let located = file.map(Path::to_path_buf);
        DocumentSettings {
            path,
            located,
            project,
            linter,
            is_included,
            is_definitions,
            definitions,
        }
    }

    /// The `.editorconfig` files formatting reads.
    pub(crate) fn editorconfigs(&self) -> Arc<Editorconfigs> {
        Arc::clone(&self.lookups.read().editorconfigs)
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
        let stamp = {
            let lookups = self.lookups.read();
            if let Some(project) = lookups.cached(directory) {
                return (Arc::clone(project), None);
            }
            Stamp { generation: lookups.generation, definition_edits: lookups.definition_edits }
        };
        let files = &self.files;
        let loaded = |project| {
            Arc::new(LoadedProject::new(project, files.clone(), Arc::clone(&self.resolver)))
        };
        let defaults = |root| loaded(Project::defaults(root, files.clone()));
        if let Some(given) = &self.given {
            let text = given.text.read().clone();
            let (project, error) =
                match Project::from_config(&text, given.root.clone(), files.clone()) {
                    Ok(project) => (loaded(project), None),
                    Err(error) => (defaults(given.root.clone()), Some(error)),
                };
            return self.keep(stamp, None, None, project, error);
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
        self.keep(stamp, directory, config_path, project, error)
    }

    /// Keeps `project`, which a lookup that started at `stamp` found, unless
    /// the cache was cleared since, or it reads definition files and one
    /// changed since.
    fn keep(
        &self,
        stamp: Stamp,
        directory: Option<&Path>,
        config_path: Option<PathBuf>,
        project: Arc<LoadedProject>,
        error: Option<ConfigError>,
    ) -> (Arc<LoadedProject>, Option<LoadError>) {
        let mut lookups = self.lookups.write();
        let reads_definitions = project.project().reads_definition_files();
        if lookups.generation != stamp.generation
            || (reads_definitions && lookups.definition_edits != stamp.definition_edits)
        {
            return (project, None);
        }
        let (kept, reported) = lookups.keep(directory, config_path, project, error);
        let generation = stamp.generation;
        (kept, reported.map(|problem| LoadError { error: problem, generation }))
    }

    /// Bumped by each [`clear`](Self::clear).
    pub(crate) fn generation(&self) -> u64 {
        self.lookups.read().generation
    }

    /// Whether a change to the file at `path`, which created it when
    /// `is_created`, can change a project, but by the definition files it
    /// lists.
    pub(crate) fn is_settings_file(&self, path: &Path, is_created: bool) -> bool {
        let Some(name) = path.file_name() else {
            return false;
        };
        let lookups = self.lookups.read();
        name == CONFIG_FILE_NAME
            || lookups.config_names.contains(name)
            || (is_created
                && lookups.is_broken
                && path.extension().is_some_and(|extension| {
                    ["json", "luau", "lua"].iter().any(|known| extension == *known)
                }))
    }

    /// Forgets every lookup and every `.editorconfig` read.
    pub(crate) fn clear(&self) {
        let mut lookups = self.lookups.write();
        let generation = lookups.generation + 1;
        *lookups = Lookups { generation, ..Lookups::default() };
    }

    /// Forgets every `.editorconfig` read.
    pub(crate) fn clear_editorconfigs(&self) {
        self.lookups.write().editorconfigs = Arc::default();
    }

    /// Forgets every project a change to the definition file at `located`, a
    /// path [`located_path`](luck_config::located_path) gives, can change, as
    /// [`LoadedProject::reads_definition`] says, and says whether there was
    /// one.
    pub(crate) fn definition_changed(&self, located: &Path) -> bool {
        let mut lookups = self.lookups.write();
        lookups.definition_edits += 1;
        let cached = lookups.by_directory.len() + lookups.by_config.len();
        lookups.by_directory.retain(|_, project| !project.reads_definition(located));
        lookups.by_config.retain(|_, project| !project.reads_definition(located));
        let is_untitled_reading =
            lookups.untitled.as_ref().is_some_and(|project| project.reads_definition(located));
        if is_untitled_reading {
            lookups.untitled = None;
        }
        is_untitled_reading || lookups.by_directory.len() + lookups.by_config.len() != cached
    }
}

/// When a lookup started.
#[derive(Debug, Clone, Copy)]
struct Stamp {
    generation: u64,
    definition_edits: u64,
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

    use luck_config::located_path;
    use luck_resolver::FileSystemOs;

    use super::*;

    fn cache() -> SettingsCache {
        let files = Files::new(None);
        let resolver = Arc::new(ResolverGeneric::new_with_file_system(files.clone()));
        SettingsCache::new(files, resolver, None)
    }

    #[test]
    fn a_document_with_no_config_takes_the_defaults_rooted_at_its_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("foo.luau");
        let cache = cache();
        let (project, error) = cache.project_for(Some(&file), None);
        assert!(error.is_none());
        assert!(!project.project().definitions().stdlib().library().has_roblox);
        if find_config(&FileSystemOs, directory.path()).is_none() {
            assert_eq!(project.project().root(), directory.path());
        }
        let (untitled, _) = cache.project_for(None, None);
        assert!(!untitled.project().definitions().stdlib().library().has_roblox);
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
        let cache = cache();

        let (project, _) = cache.project_for(Some(&first), None);
        assert!(project.project().definitions().stdlib().library().has_roblox);
        assert!(Arc::ptr_eq(&project, &cache.project_for(Some(&nested), None).0));

        fs::write(&config_path, "{}").expect("rewrite luck.json");
        let (cached, _) = cache.project_for(Some(&first), None);
        assert!(cached.project().definitions().stdlib().library().has_roblox);
        let generation = cache.generation();
        cache.clear();
        assert_ne!(cache.generation(), generation, "a clear starts a new generation");
        let (reloaded, _) = cache.project_for(Some(&first), None);
        assert!(!reloaded.project().definitions().stdlib().library().has_roblox);
    }

    #[test]
    fn a_broken_config_is_reported_once_and_its_files_take_the_defaults() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), r#"{ "definitions": ["@bogus"] }"#)
            .expect("write luck.json");
        let file = directory.path().join("a.luau");
        let cache = cache();

        let (project, first_error) = cache.project_for(Some(&file), None);
        let error = first_error.expect("the error comes with the first lookup");
        let ConfigError::File(problem) = &error.error else { panic!("{error:?}") };
        assert_eq!(problem.diags[0].code, "E014");
        assert_eq!(error.generation, cache.generation());
        assert!(!project.project().definitions().stdlib().library().has_roblox);
        assert!(cache.project_for(Some(&file), None).1.is_none(), "reported once");

        let other = directory.path().join("other.json");
        assert!(cache.is_settings_file(&other, true), "a chain may extend it once it's created");
        assert!(!cache.is_settings_file(&other, false));
        cache.clear();
        assert!(!cache.is_settings_file(&other, true));
    }

    #[test]
    fn a_file_a_project_extends_is_a_settings_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("shared.json"), "{}").expect("write shared.json");
        fs::write(directory.path().join("app.d.luau"), "declare app: {}")
            .expect("write app.d.luau");
        fs::write(
            directory.path().join("luck.json"),
            r#"{ "extends": ["shared.json"], "definitions": ["app.d.luau"] }"#,
        )
        .expect("write luck.json");
        let cache = cache();
        let sourcemap = directory.path().join("sourcemap.json");
        let shared = directory.path().join("shared.json");
        assert!(!cache.is_settings_file(&shared, false));

        let _ = cache.project_for(Some(&directory.path().join("a.luau")), None);
        assert!(cache.is_settings_file(&shared, false));
        assert!(cache.is_settings_file(&directory.path().join("luck.json"), false));
        assert!(!cache.is_settings_file(&sourcemap, false), "Rojo rewrites it on every change");
        assert!(
            !cache.is_settings_file(&directory.path().join(".luaurc"), false),
            "a `.luaurc` changes how requires resolve, not settings"
        );
        assert!(
            !cache.is_settings_file(&directory.path().join("app.d.luau"), false),
            "a definition file changes only the projects that list it"
        );
    }

    #[test]
    fn a_definition_file_s_change_forgets_only_the_projects_that_list_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        for (name, text) in [
            ("listing/app.d.luau", "declare app: {}"),
            ("listing/luck.json", r#"{ "definitions": ["app.d.luau"] }"#),
            ("other/luck.json", "{}"),
        ] {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
            fs::write(path, text).expect("write a config");
        }
        let (listing, other) =
            (directory.path().join("listing/a.luau"), directory.path().join("other/a.luau"));
        let cache = cache();
        let (listing_project, _) = cache.project_for(Some(&listing), None);
        let (other_project, _) = cache.project_for(Some(&other), None);

        let located = |name: &str| located_path(&FileSystemOs, &directory.path().join(name));
        assert!(!cache.definition_changed(&located("listing/unlisted.d.luau")));
        assert!(cache.definition_changed(&located("listing/app.d.luau")));

        assert!(!Arc::ptr_eq(&listing_project, &cache.project_for(Some(&listing), None).0));
        assert!(Arc::ptr_eq(&other_project, &cache.project_for(Some(&other), None).0));
    }

    #[test]
    fn creating_a_definition_file_a_glob_takes_forgets_the_project_with_the_glob() {
        let directory = tempfile::tempdir().expect("tempdir");
        for (name, text) in [
            ("globbing/luck.json", r#"{ "definitions": ["types/**/*.d.luau"] }"#),
            ("other/luck.json", "{}"),
        ] {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
            fs::write(path, text).expect("write a config");
        }
        let (globbing, other) =
            (directory.path().join("globbing/a.luau"), directory.path().join("other/a.luau"));
        let cache = cache();
        let (globbing_project, _) = cache.project_for(Some(&globbing), None);
        let (other_project, _) = cache.project_for(Some(&other), None);
        assert_eq!(globbing_project.project().definitions().sources(), []);

        let created = directory.path().join("globbing/types/ui/app.d.luau");
        fs::create_dir_all(created.parent().expect("a directory")).expect("mkdir");
        fs::write(&created, "declare app: {}").expect("write app.d.luau");
        let located = |path: &Path| located_path(&FileSystemOs, path);
        assert!(!cache.definition_changed(&located(&directory.path().join("other/app.d.luau"))));
        assert!(cache.definition_changed(&located(&created)));

        let (reloaded, _) = cache.project_for(Some(&globbing), None);
        assert_eq!(reloaded.project().definitions().sources().len(), 1);
        assert!(Arc::ptr_eq(&other_project, &cache.project_for(Some(&other), None).0));
    }

    #[test]
    fn every_project_resolves_through_the_session_s_resolver() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(directory.path().join("sub")).expect("mkdir sub");
        fs::write(directory.path().join("sub/luck.json"), "{}").expect("write luck.json");
        let main = directory.path().join("main.luau");
        let cache = cache();
        let (project, _) = cache.project_for(Some(&main), None);
        let (nested, _) = cache.project_for(Some(&directory.path().join("sub/a.luau")), None);
        assert!(!Arc::ptr_eq(&project, &nested));
        let resolver = project.resolver();
        assert!(ptr::eq(resolver, nested.resolver()));
        assert!(resolver.resolve(&main, "./late").is_err(), "nothing named `late` exists yet");

        fs::write(directory.path().join("late.luau"), "return {}\n").expect("write late.luau");
        assert!(resolver.resolve(&main, "./late").is_err(), "the listing is cached");
        resolver.clear_cache();
        assert!(resolver.resolve(&main, "./late").is_ok(), "the cleared resolver reads the disk");
    }
}
