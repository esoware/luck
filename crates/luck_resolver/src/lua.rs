use std::path::PathBuf;

use crate::cache::Cache;
use crate::file_system::FileSystem;
use crate::shared_cache::SharedCache;
use crate::{Resolution, ResolveError};

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
    pub(crate) fn new(search_paths: Vec<String>, root: PathBuf) -> Self {
        Self {
            search_paths: if search_paths.is_empty() {
                DEFAULT_SEARCH_PATHS.map(str::to_owned).to_vec()
            } else {
                search_paths
            },
            root,
            resolved: SharedCache::default(),
        }
    }

    pub(crate) fn clear(&self) {
        self.resolved.clear();
    }

    pub(crate) fn resolve(
        &self,
        cache: &Cache<impl FileSystem>,
        module: &str,
    ) -> Result<Resolution, ResolveError> {
        if let Some(path) = self.resolved.get(module) {
            return Ok(Resolution { path, is_self_alias_shadowed: false });
        }
        let dotted_path = module.replace('.', "/");
        // A leading dot must not turn a relative template into a rooted path.
        let module_path = dotted_path.trim_start_matches('/');
        let found = self.search_paths.iter().find_map(|template| {
            let candidate = self.root.join(template.replace('?', module_path));
            cache.is_file(&candidate).then(|| cache.canonical_file(&candidate))
        });
        let Some(path) = found else {
            return Err(ResolveError::NotFound(
                self.search_paths
                    .iter()
                    .map(|template| PathBuf::from(template.replace('?', module_path)))
                    .collect(),
            ));
        };
        Ok(Resolution {
            path: self.resolved.insert(module.to_owned(), path),
            is_self_alias_shadowed: false,
        })
    }
}
