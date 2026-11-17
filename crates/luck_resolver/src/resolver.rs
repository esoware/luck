use alloc::borrow::Cow;
use std::path::Path;

use luck_core::Diagnostic;
use luck_token::{LuaVersion, Span};
use rustc_hash::FxHashMap;

use crate::luau::ModuleProbes;
use crate::luaurc::AliasCache;
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
/// build.
///
/// Lua caches what each require string resolved to, and Luau the `.luaurc`
/// alias tables it discovers, what each module path holds on disk, and each
/// module's canonical path. A missing or malformed file is cached too.
#[derive(Debug)]
pub struct Resolver<'config> {
    scheme: Scheme<'config>,
}

#[derive(Debug)]
enum Scheme<'config> {
    Lua {
        search_paths: Cow<'config, [String]>,
        project_root: &'config Path,
        /// What each require string that resolved names, which only it
        /// decides.
        resolved: FxHashMap<String, String>,
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
        } else if search_paths.is_empty() {
            let default_paths = DEFAULT_SEARCH_PATHS.iter().map(ToString::to_string).collect();
            Scheme::Lua {
                search_paths: Cow::Owned(default_paths),
                project_root,
                resolved: FxHashMap::default(),
            }
        } else {
            Scheme::Lua {
                search_paths: Cow::Borrowed(search_paths),
                project_root,
                resolved: FxHashMap::default(),
            }
        };
        Self { scheme }
    }

    /// # Errors
    ///
    /// Returns the diagnostic explaining why `request` names no module, or more
    /// than one.
    pub fn resolve(&mut self, request: &ResolveRequest<'_>) -> Result<ResolvedModule, Diagnostic> {
        match &mut self.scheme {
            Scheme::Lua { search_paths, project_root, resolved } => {
                if let Some(path) = resolved.get(request.module) {
                    return Ok(ResolvedModule { path: path.clone(), warnings: Vec::new() });
                }
                let module = lua::resolve(search_paths, project_root, request)?;
                resolved.insert(request.module.to_owned(), module.path.clone());
                Ok(module)
            }
            Scheme::Luau(aliases, probes) => luau::resolve(aliases, probes, request),
        }
    }
}
