use alloc::borrow::Cow;
use std::path::Path;

use luck_core::Diagnostic;
use luck_token::{LuaVersion, Span};

use crate::file_system::FileSystem;
use crate::luau::ModuleProbes;
use crate::luaurc::AliasCache;
use crate::shared_cache::SharedCache;
use crate::{lua, luau};

const DEFAULT_SEARCH_PATHS: &[&str] = &["?.lua", "?/init.lua"];

#[derive(Debug)]
pub struct ResolvedModule {
    /// In the [`normalize_path`](crate::normalize_path) form.
    pub path: String,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolveRequest<'request> {
    /// The require string, e.g. `"foo.bar"`, `"./sibling"`, or `"@alias/mod"`.
    pub module: &'request str,
    /// In the [`normalize_path`](crate::normalize_path) form.
    pub from_file: &'request str,
    pub span: Span,
}

/// Caches what it finds and never rereads it, so make a fresh resolver per
/// build. Threads share one, and each thing it finds is looked up once for
/// all of them.
///
/// It lists each directory a require looks in once, and answers from that
/// listing which files exist and their canonical paths. Lua caches what each
/// require string resolved to, and Luau the `.luaurc` alias tables it
/// discovers and what each module path holds on disk. A missing or malformed
/// file is cached too.
#[derive(Debug)]
pub struct Resolver<'config> {
    scheme: Scheme<'config>,
}

#[derive(Debug)]
enum Scheme<'config> {
    Lua {
        search_paths: Cow<'config, [String]>,
        project_root: &'config Path,
        files: FileSystem,
        /// What each require string that resolved names, which only it
        /// decides.
        resolved: SharedCache<String, String>,
    },
    Luau(AliasCache, ModuleProbes),
}

impl<'config> Resolver<'config> {
    /// Lua 5.x tries each of `search_paths`, or `?.lua` and `?/init.lua`
    /// when it's empty, against `project_root`. Luau ignores both and
    /// resolves from the requiring file.
    #[must_use]
    pub fn new(
        version: LuaVersion,
        search_paths: &'config [String],
        project_root: &'config Path,
    ) -> Self {
        let scheme = if version.is_luau() {
            Scheme::Luau(AliasCache::default(), ModuleProbes::default())
        } else {
            Scheme::Lua {
                search_paths: if search_paths.is_empty() {
                    Cow::Owned(DEFAULT_SEARCH_PATHS.iter().map(ToString::to_string).collect())
                } else {
                    Cow::Borrowed(search_paths)
                },
                project_root,
                files: FileSystem::default(),
                resolved: SharedCache::default(),
            }
        };
        Self { scheme }
    }

    /// # Errors
    ///
    /// Returns the diagnostic explaining why `request` names no module, or more
    /// than one.
    pub fn resolve(&self, request: &ResolveRequest<'_>) -> Result<ResolvedModule, Diagnostic> {
        match &self.scheme {
            Scheme::Lua { search_paths, project_root, files, resolved } => {
                if let Some(path) = resolved.get(request.module) {
                    return Ok(ResolvedModule { path, warnings: Vec::new() });
                }
                let module = lua::resolve(files, search_paths, project_root, request)?;
                resolved.insert(request.module.to_owned(), module.path.clone());
                Ok(module)
            }
            Scheme::Luau(aliases, probes) => luau::resolve(aliases, probes, request),
        }
    }
}
