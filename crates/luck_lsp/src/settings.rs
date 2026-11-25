//! Discovery walks up from the document's directory.
//!
//! Results, misses and configs that don't load included, stay cached until a
//! watched config file changes. A lookup reads the disk, so only workers make
//! one.

use std::ffi::OsString;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use luck_config::{CONFIG_FILE_NAME, EDITORCONFIG_FILE_NAME, Editorconfigs, Project, find_config};
use luck_diagnostics::FileDiagnostics;
use luck_formatter::FormatOptions;
use luck_linter::Linter;
use luck_resolver::{FileSystem as _, FileSystemOs, Resolver};
use luck_stdlib::Stdlib;
use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHashSet};

/// What a document's path and the project around it fix, which every
/// version of the document shares until the settings change.
#[derive(Debug)]
pub(crate) struct DocumentSettings {
    /// Canonical, or `None` for a document with no file.
    pub path: Option<PathBuf>,
    pub project: Arc<LoadedProject>,
    /// The standard library of the project's variant, with its std
    /// definitions over it.
    pub stdlib: Stdlib,
    /// The linter of the project's lint settings, with the overrides the
    /// path matches. Publishing, code actions, and fix-all all lint with it,
    /// so none of them fixes a diagnostic the user was never shown.
    pub linter: Arc<Linter>,
    pub format: FormatOptions,
    /// Whether the project's `include` and `exclude` take the document in.
    /// One they leave out gets no diagnostics and no fixes.
    pub is_included: bool,
}

/// A project, and the resolver its files' requires go through, so each
/// request answers from what the ones before it read.
#[derive(Debug)]
pub(crate) struct LoadedProject {
    project: Project,
    resolver: Resolver,
}

impl LoadedProject {
    pub(crate) fn new(project: Project) -> Self {
        Self { project, resolver: Resolver::new() }
    }

    pub(crate) const fn project(&self) -> &Project {
        &self.project
    }

    pub(crate) const fn resolver(&self) -> &Resolver {
        &self.resolver
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
    /// The settings of the document at `file`, a path [`settings_path`]
    /// gives, or of an untitled document, under `project`, the one
    /// [`Self::project_for`] gives it.
    pub(crate) fn document_settings(
        &self,
        project: Arc<LoadedProject>,
        file: Option<&Path>,
    ) -> DocumentSettings {
        let editorconfigs = Arc::clone(&self.lookups.read().editorconfigs);
        let loaded = project.project();
        let stdlib = loaded.stdlib().clone();
        let linter = loaded.linter_for(file);
        let format = loaded.format_options(file, Some(&editorconfigs));
        let path = file.map(|located| FileSystemOs.canonicalize_existing(located));
        let is_included =
            path.as_deref().is_none_or(|canonical| loaded.filter().includes_canonical(canonical));
        DocumentSettings { path, project, stdlib, linter, format, is_included }
    }

    /// The project of the nearest `luck.json` above the file at
    /// `document_path`, whose directory is canonical, so each directory and
    /// config has one spelling, or the defaults rooted at its directory when
    /// there's none, and why that `luck.json` doesn't load, if this is the
    /// first lookup to find it. A `luck.json` that doesn't load gives the
    /// defaults rooted at its directory.
    ///
    /// `spelled` is the document's path as the client spells it. A load error
    /// names its file under the same directories, since a client knows a file
    /// only by its own spelling.
    pub(crate) fn project_for(
        &self,
        document_path: Option<&Path>,
        spelled: Option<&Path>,
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
                Err(mut problem) => {
                    if let Some(document) = spelled {
                        problem.path = spelled_beside(&problem.path, document);
                    }
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

/// `spelled`, a document's path as the client spells it, under its
/// directory's canonical path.
///
/// A document's project and settings follow this, as `luck lint`'s follow
/// the path it's given, so a symlink takes the project of its own directory
/// and not its target's.
pub(crate) fn settings_path(spelled: &Path) -> PathBuf {
    match (spelled.parent(), spelled.file_name()) {
        (Some(directory), Some(name)) => FileSystemOs.canonicalize_existing(directory).join(name),
        _ => FileSystemOs.canonicalize_existing(spelled),
    }
}

/// `path`, a canonical path, as the client spells the files near `document`.
///
/// That's under the deepest directory above `document` whose canonical path
/// holds it, so a client that reached `document` through a symlink or a
/// short name knows the file, or `path` itself when none holds it.
fn spelled_beside(path: &Path, document: &Path) -> PathBuf {
    document
        .ancestors()
        .skip(1)
        .find_map(|directory| {
            let inside = path.strip_prefix(FileSystemOs.canonicalize_existing(directory)).ok()?;
            Some(directory.join(inside))
        })
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::ptr;

    use luck_token::Variant;

    use super::*;

    #[test]
    fn a_document_with_no_config_takes_the_defaults_rooted_at_its_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("foo.lua");
        let cache = SettingsCache::default();
        let (project, error) = cache.project_for(Some(&file), None);
        assert!(error.is_none());
        assert_eq!(project.project().variant(), Variant::Standalone);
        if find_config(directory.path()).is_none() {
            assert_eq!(project.project().root(), directory.path());
        }
        let (untitled, _) = cache.project_for(None, None);
        assert_eq!(untitled.project().variant(), Variant::Standalone);
        assert!(Arc::ptr_eq(&untitled, &cache.project_for(None, None).0));
    }

    #[test]
    fn cache_keeps_projects_until_cleared() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, r#"{ "variant": "roblox" }"#).expect("write luck.json");
        fs::create_dir_all(directory.path().join("sub")).expect("mkdir sub");
        let first = directory.path().join("first.lua");
        let nested = directory.path().join("sub/nested.lua");
        let cache = SettingsCache::default();

        let (project, _) = cache.project_for(Some(&first), None);
        assert_eq!(project.project().variant(), Variant::Roblox);
        assert!(Arc::ptr_eq(&project, &cache.project_for(Some(&nested), None).0));

        fs::write(&config_path, r#"{ "variant": "standalone" }"#).expect("rewrite luck.json");
        let (cached, _) = cache.project_for(Some(&first), None);
        assert_eq!(cached.project().variant(), Variant::Roblox);
        let generation = cache.generation();
        cache.clear();
        assert_ne!(cache.generation(), generation, "a clear starts a new generation");
        let (reloaded, _) = cache.project_for(Some(&first), None);
        assert_eq!(reloaded.project().variant(), Variant::Standalone);
    }

    #[test]
    fn a_broken_config_is_reported_once_and_its_files_take_the_defaults() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), r#"{ "variant": "bogus" }"#)
            .expect("write luck.json");
        let file = directory.path().join("a.lua");
        let cache = SettingsCache::default();

        let (project, first_error) = cache.project_for(Some(&file), None);
        let error = first_error.expect("the error comes with the first lookup");
        assert_eq!(error.file.diags[0].code, "E014");
        assert_eq!(error.generation, cache.generation());
        assert_eq!(project.project().variant(), Variant::Standalone);
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
        let cache = SettingsCache::default();
        let sourcemap = directory.path().join("sourcemap.json");
        let shared = directory.path().join("shared.json");
        assert!(!cache.is_settings_file(&shared));

        let _ = cache.project_for(Some(&directory.path().join("a.lua")), None);
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
        let (project, _) = cache.project_for(Some(&main), None);
        let resolver = project.resolver();
        assert!(ptr::eq(resolver, cache.project_for(Some(&main), None).0.resolver()));
        assert!(resolver.resolve(&main, "./late").is_err(), "nothing named `late` exists yet");

        fs::write(directory.path().join("late.lua"), "return {}\n").expect("write late.lua");
        assert!(resolver.resolve(&main, "./late").is_err(), "the listing is cached");
        cache.clear_resolvers();
        assert!(resolver.resolve(&main, "./late").is_ok(), "the cleared resolver reads the disk");
    }
}
