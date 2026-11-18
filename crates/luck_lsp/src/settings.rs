//! Discovery walks up from the document's directory. Results, misses and
//! configs that don't load included, stay cached until a watched config file
//! changes.

use alloc::sync::Arc;
use core::mem;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use luck_config::{CONFIG_FILE_NAME, Editorconfigs, Project, find_config};
use luck_core::Diagnostic;
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::RwLock;
use tokio::task::spawn_blocking;

/// The other files whose change can change a document's settings.
const SETTINGS_FILE_NAMES: [&str; 2] = [".editorconfig", ".luaurc"];

#[derive(Debug, Default)]
pub(crate) struct SettingsCache {
    /// One lock for all of them, so a reader never sees a directory without
    /// its project.
    lookups: RwLock<Lookups>,
}

#[derive(Debug)]
struct Lookups {
    /// Shared by every document without a file path.
    untitled: Arc<Project>,
    /// By the directory a document is in.
    by_directory: FxHashMap<PathBuf, Arc<Project>>,
    /// By the path of their `luck.json`, so documents in several of a
    /// project's directories share it.
    by_config: FxHashMap<PathBuf, Arc<Project>>,
    /// The `.editorconfig` files formatting has read, each read once until a
    /// watched config file changes.
    editorconfigs: Arc<Editorconfigs>,
    /// The name of every file a project read, beside `luck.json`, which a
    /// watched change has to touch to matter.
    config_names: FxHashSet<OsString>,
    /// Each config file a reported load error is in. While there's one, any
    /// JSON file a broken chain might name matters.
    broken: Vec<PathBuf>,
}

impl Default for Lookups {
    fn default() -> Self {
        Self {
            untitled: Arc::new(Project::defaults(PathBuf::from("."))),
            by_directory: FxHashMap::default(),
            by_config: FxHashMap::default(),
            editorconfigs: Arc::default(),
            config_names: FxHashSet::default(),
            broken: Vec::new(),
        }
    }
}

/// A document's project, and why its `luck.json` didn't load the first time
/// a document asks, so the problem is reported once.
#[derive(Debug)]
pub(crate) struct Discovered {
    pub project: Arc<Project>,
    pub error: Option<Diagnostic>,
}

impl SettingsCache {
    /// The project of the nearest `luck.json` above the document at
    /// `document_path`, or the defaults rooted at its directory when there's
    /// none. A `luck.json` that doesn't load gives the defaults rooted at its
    /// directory, with the error.
    pub(crate) async fn project_for(&self, document_path: Option<&Path>) -> Discovered {
        let Some(directory) = document_path.and_then(Path::parent) else {
            return Discovered {
                project: Arc::clone(&self.lookups.read().await.untitled),
                error: None,
            };
        };
        if let Some(project) = self.lookups.read().await.by_directory.get(directory) {
            return Discovered { project: Arc::clone(project), error: None };
        }
        // Discovery stats every directory up to the root, and the defaults'
        // filter canonicalizes theirs.
        let search = directory.to_path_buf();
        let found = spawn_blocking(move || {
            find_config(&search).ok_or_else(|| Arc::new(Project::defaults(search)))
        })
        .await;
        let config_path = match found {
            Ok(Ok(config_path)) => config_path,
            Ok(Err(defaults)) => {
                let by_directory = &mut self.lookups.write().await.by_directory;
                by_directory.insert(directory.to_path_buf(), Arc::clone(&defaults));
                return Discovered { project: defaults, error: None };
            }
            // A lookup that dies is tried again by the next document.
            Err(_) => {
                let project = Arc::clone(&self.lookups.read().await.untitled);
                return Discovered { project, error: None };
            }
        };
        let shared = self.lookups.read().await.by_config.get(&config_path).cloned();
        if let Some(project) = shared {
            let by_directory = &mut self.lookups.write().await.by_directory;
            by_directory.insert(directory.to_path_buf(), Arc::clone(&project));
            return Discovered { project, error: None };
        }
        // Loading reads every file in the `extends` chain.
        let loaded_path = config_path.clone();
        let loaded = spawn_blocking(move || match Project::load(&loaded_path) {
            Ok(project) => (Arc::new(project), None),
            Err(diag) => {
                let root = loaded_path.parent().map_or_else(PathBuf::new, Path::to_path_buf);
                (Arc::new(Project::defaults(root)), Some(diag))
            }
        })
        .await;
        let Ok((project, error)) = loaded else {
            let untitled = Arc::clone(&self.lookups.read().await.untitled);
            return Discovered { project: untitled, error: None };
        };
        let mut lookups = self.lookups.write().await;
        // Another document may have loaded it meanwhile, and reported it.
        if let Some(known) = lookups.by_config.get(&config_path).cloned() {
            lookups.by_directory.insert(directory.to_path_buf(), Arc::clone(&known));
            drop(lookups);
            return Discovered { project: known, error: None };
        }
        let names = project.files().iter().filter_map(|file| file.file_name()).map(OsString::from);
        lookups.config_names.extend(names);
        if let Some(diag) = &error {
            let file = PathBuf::from(&diag.file_path);
            lookups.config_names.extend(file.file_name().map(OsString::from));
            lookups.broken.push(file);
        }
        lookups.by_config.insert(config_path, Arc::clone(&project));
        lookups.by_directory.insert(directory.to_path_buf(), Arc::clone(&project));
        drop(lookups);
        Discovered { project, error }
    }

    pub(crate) async fn editorconfigs(&self) -> Arc<Editorconfigs> {
        Arc::clone(&self.lookups.read().await.editorconfigs)
    }

    /// Whether a change to the file at `path` can change a document's
    /// settings.
    pub(crate) async fn is_settings_file(&self, path: &Path) -> bool {
        let Some(name) = path.file_name() else {
            return false;
        };
        let lookups = self.lookups.read().await;
        name == CONFIG_FILE_NAME
            || SETTINGS_FILE_NAMES.iter().any(|settings_name| name == *settings_name)
            || lookups.config_names.contains(name)
            || (!lookups.broken.is_empty()
                && path.extension().is_some_and(|extension| extension == "json"))
    }

    /// Forgets every lookup and every `.editorconfig` read, and returns each
    /// config file a load error was reported in.
    pub(crate) async fn clear(&self) -> Vec<PathBuf> {
        let Lookups { broken, .. } = mem::take(&mut *self.lookups.write().await);
        broken
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_core::LuaTarget;

    use super::*;

    #[tokio::test]
    async fn a_document_with_no_config_takes_the_defaults_rooted_at_its_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("foo.lua");
        let cache = SettingsCache::default();
        let Discovered { project, error } = cache.project_for(Some(&file)).await;
        assert!(error.is_none());
        assert_eq!(project.target_for(Some(&file)), LuaTarget::Lua54);
        if find_config(directory.path()).is_none() {
            assert_eq!(project.root(), directory.path());
        }
        let untitled = cache.project_for(None).await.project;
        assert_eq!(untitled.target_for(None), LuaTarget::Lua54);
    }

    #[tokio::test]
    async fn cache_keeps_projects_until_cleared() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, r#"{ "lua": "lua53" }"#).expect("write luck.json");
        fs::create_dir_all(directory.path().join("sub")).expect("mkdir sub");
        let first = directory.path().join("first.lua");
        let nested = directory.path().join("sub/nested.lua");
        let cache = SettingsCache::default();

        let project = cache.project_for(Some(&first)).await.project;
        assert_eq!(project.target_for(Some(&first)), LuaTarget::Lua53);
        assert!(Arc::ptr_eq(&project, &cache.project_for(Some(&nested)).await.project));

        fs::write(&config_path, r#"{ "lua": "lua51" }"#).expect("rewrite luck.json");
        let cached = cache.project_for(Some(&first)).await.project;
        assert_eq!(cached.target_for(Some(&first)), LuaTarget::Lua53);
        cache.clear().await;
        let reloaded = cache.project_for(Some(&first)).await.project;
        assert_eq!(reloaded.target_for(Some(&first)), LuaTarget::Lua51);
    }

    #[tokio::test]
    async fn a_broken_config_is_reported_once_and_its_files_take_the_defaults() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), r#"{ "lua": "lua99" }"#)
            .expect("write luck.json");
        let file = directory.path().join("a.lua");
        let cache = SettingsCache::default();

        let Discovered { project, error } = cache.project_for(Some(&file)).await;
        let diag = error.expect("the error comes with the first lookup");
        assert_eq!(diag.code, "E014");
        assert_eq!(project.target_for(Some(&file)), LuaTarget::Lua54);
        assert!(cache.project_for(Some(&file)).await.error.is_none(), "reported once");

        assert!(cache.is_settings_file(&directory.path().join("other.json")).await);
        let broken = cache.clear().await;
        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(!cache.is_settings_file(&directory.path().join("other.json")).await);
    }

    #[tokio::test]
    async fn a_file_a_project_extends_is_a_settings_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("shared.json"), "{}").expect("write shared.json");
        fs::write(directory.path().join("luck.json"), r#"{ "extends": ["shared.json"] }"#)
            .expect("write luck.json");
        let cache = SettingsCache::default();
        let sourcemap = directory.path().join("sourcemap.json");
        let shared = directory.path().join("shared.json");
        assert!(!cache.is_settings_file(&shared).await);

        let _discovered = cache.project_for(Some(&directory.path().join("a.lua"))).await;
        assert!(cache.is_settings_file(&shared).await);
        assert!(cache.is_settings_file(&directory.path().join("luck.json")).await);
        assert!(cache.is_settings_file(&directory.path().join(".editorconfig")).await);
        assert!(!cache.is_settings_file(&sourcemap).await, "Rojo rewrites it on every change");
    }
}
