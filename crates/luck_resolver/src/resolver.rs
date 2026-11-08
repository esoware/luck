use std::path::Path;

use luck_core::Diagnostic;
use luck_token::{LuaVersion, Span};

use crate::luaurc::AliasCache;
use crate::{lua, luau};

/// A resolved module path, with any warnings raised while resolving it.
#[derive(Debug)]
pub struct ResolvedModule {
    /// In the [`normalize_path`](luck_core::normalize_path) form.
    pub path: String,
    pub warnings: Vec<Diagnostic>,
}

/// One `require()` to resolve.
#[derive(Debug, Clone, Copy)]
pub struct ResolveRequest<'request> {
    /// The require string, e.g. `"foo.bar"`, `"./sibling"`, or `"@alias/mod"`.
    pub module: &'request str,
    /// The file containing the require, in the
    /// [`normalize_path`](luck_core::normalize_path) form.
    pub from_file: &'request str,
    /// Byte span of the require call, for diagnostics.
    pub span: Span,
}

/// Resolves the `require()` strings of one build.
///
/// Luau caches the `.luaurc` alias tables it discovers. Cached data, including
/// a missing or malformed file, is never reread, so make a fresh resolver per
/// build.
#[derive(Debug)]
pub struct Resolver<'config> {
    scheme: Scheme<'config>,
}

#[derive(Debug)]
enum Scheme<'config> {
    Lua { search_paths: &'config [String], project_root: &'config Path },
    Luau(AliasCache),
}

impl<'config> Resolver<'config> {
    /// A resolver for `version` source.
    ///
    /// Lua 5.x tries each of `search_paths` against `project_root`. Luau
    /// ignores both and resolves from the requiring file.
    #[must_use]
    pub fn new(
        version: LuaVersion,
        search_paths: &'config [String],
        project_root: &'config Path,
    ) -> Self {
        let scheme = if version.is_luau() {
            Scheme::Luau(AliasCache::default())
        } else {
            Scheme::Lua { search_paths, project_root }
        };
        Self { scheme }
    }

    /// Resolves one `require()`.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic explaining why `request` names no module, or more
    /// than one.
    pub fn resolve(&mut self, request: &ResolveRequest<'_>) -> Result<ResolvedModule, Diagnostic> {
        match &mut self.scheme {
            Scheme::Lua { search_paths, project_root } => {
                lua::resolve(search_paths, project_root, request)
            }
            Scheme::Luau(aliases) => luau::resolve(aliases, request),
        }
    }
}
