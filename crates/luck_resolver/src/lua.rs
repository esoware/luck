use std::path::PathBuf;
use std::sync::Arc;

use crate::ResolveError;
use crate::file_system::FileSystem;
use crate::fs_cache::FsCache;
use crate::shared_cache::{Generation, SharedCache};

const DEFAULT_SEARCH_PATHS: [&str; 2] = ["?.lua", "?/init.lua"];

/// Lua 5.x's search: the module name, with its dots as slashes, in place of
/// each template's `?`, and the first file that names.
#[derive(Debug)]
pub(crate) struct Lua {
    search_paths: Vec<String>,
    root: PathBuf,
    /// What each require string that resolved names, which only it decides.
    resolved: SharedCache<String, PathBuf>,
}

impl Lua {
    pub(crate) fn new(
        search_paths: Vec<String>,
        root: PathBuf,
        generation: &Arc<Generation>,
    ) -> Self {
        Self {
            search_paths: if search_paths.is_empty() {
                DEFAULT_SEARCH_PATHS.map(str::to_owned).to_vec()
            } else {
                search_paths
            },
            root,
            resolved: SharedCache::new(generation),
        }
    }

    pub(crate) fn clear(&self) {
        self.resolved.clear();
    }

    pub(crate) fn resolve(
        &self,
        fs_cache: &FsCache<impl FileSystem>,
        module: &str,
    ) -> Result<PathBuf, ResolveError> {
        self.resolved.get_or_try_insert_with(module, || {
            let slashed_module = module.replace('.', "/");
            // A leading dot must not turn a relative template into a rooted
            // path.
            let module_path = slashed_module.trim_start_matches('/');
            let found = self.search_paths.iter().find_map(|template| {
                let candidate = self.root.join(template.replace('?', module_path));
                fs_cache.is_file(&candidate).then(|| fs_cache.canonical_file(&candidate))
            });
            found.ok_or_else(|| {
                ResolveError::NotFound(
                    self.search_paths
                        .iter()
                        .map(|template| PathBuf::from(template.replace('?', module_path)))
                        .collect(),
                )
            })
        })
    }
}
