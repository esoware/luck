use alloc::borrow::Cow;
use std::path::Path;

use luck_core::Diagnostic;
use luck_token::{LuaVersion, Span};

use crate::luau::ModuleProbes;

/// Lua 5.x search path templates used when `search_paths` is unset or empty.
pub const DEFAULT_SEARCH_PATHS: &[&str] = &["?.lua", "?/init.lua"];
use crate::luaurc::AliasCache;
use crate::{lua, luau};

#[derive(Debug)]
pub struct ResolvedModule {
    /// In the [`normalize_path`](luck_core::normalize_path) form.
    pub path: String,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolveRequest<'request> {
    /// The require string, e.g. `"foo.bar"`, `"./sibling"`, or `"@alias/mod"`.
    pub module: &'request str,
    /// In the [`normalize_path`](luck_core::normalize_path) form.
    pub from_file: &'request str,
    pub span: Span,
}

/// Luau caches the `.luaurc` alias tables it discovers and what each module
/// path holds on disk. Cached data, including a missing or malformed file, is
/// never reread, so make a fresh resolver per build.
#[derive(Debug)]
pub struct Resolver<'config> {
    scheme: Scheme<'config>,
}

#[derive(Debug)]
enum Scheme<'config> {
    Lua { search_paths: Cow<'config, [String]>, project_root: &'config Path },
    Luau(AliasCache, ModuleProbes),
}

impl<'config> Resolver<'config> {
    /// Lua 5.x tries each of `search_paths`, or [`DEFAULT_SEARCH_PATHS`] when
    /// it's empty, against `project_root`. Luau ignores both and resolves from
    /// the requiring file.
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
            Scheme::Lua { search_paths: Cow::Owned(default_paths), project_root }
        } else {
            Scheme::Lua { search_paths: Cow::Borrowed(search_paths), project_root }
        };
        Self { scheme }
    }

    /// # Errors
    ///
    /// Returns the diagnostic explaining why `request` names no module, or more
    /// than one.
    pub fn resolve(&mut self, request: &ResolveRequest<'_>) -> Result<ResolvedModule, Diagnostic> {
        match &mut self.scheme {
            Scheme::Lua { search_paths, project_root } => {
                lua::resolve(search_paths, project_root, request)
            }
            Scheme::Luau(aliases, probes) => luau::resolve(aliases, probes, request),
        }
    }
}
