use std::path::{Path, PathBuf};

use luck_diagnostics::Diagnostic;
use luck_token::{LuaVersion, Span};

use crate::cache::Cache;
use crate::error::ResolveError;
use crate::file_system::{FileSystem, FileSystemOs};
use crate::lua::Lua;
use crate::luau::Luau;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOptions {
    /// Luau resolves from the requiring file, and every other version
    /// searches `search_paths`.
    pub version: LuaVersion,
    /// Lua's templates, such as `?.lua`, where a relative one starts at
    /// `root`. Empty means `?.lua` and `?/init.lua`.
    pub search_paths: Vec<String>,
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// From [`FileSystem::canonicalize`], so two requires of one module
    /// resolve alike.
    pub path: PathBuf,
    /// Luau: `@self` named the requiring module although a `.luaurc` in
    /// effect declares an alias named `self`, which can't override it.
    pub is_self_alias_shadowed: bool,
}

impl Resolution {
    /// The W004 that warns at `span`, the require string, that `@self`
    /// shadowed a `.luaurc` alias named `self`, or `None` when it didn't.
    pub fn to_diagnostic(&self, span: Span) -> Option<Diagnostic> {
        self.is_self_alias_shadowed.then(|| Diagnostic::w004(span))
    }
}

/// A [`ResolverGeneric`] on the OS's own file system.
pub type Resolver = ResolverGeneric<FileSystemOs>;

/// Resolves requires through `Fs`, and keeps what it reads until
/// [`Self::clear_cache`], so threads that share one read each thing once.
///
/// It lists each directory a require looks in once, and answers from that
/// listing which files exist and their canonical paths. Lua keeps what each
/// require string resolved to, and Luau the `.luaurc` alias tables it
/// discovers and what each module path holds. A missing or malformed file is
/// kept too.
#[derive(Debug)]
pub struct ResolverGeneric<Fs> {
    cache: Cache<Fs>,
    scheme: Scheme,
}

#[derive(Debug)]
enum Scheme {
    Lua(Lua),
    Luau(Luau),
}

impl Resolver {
    pub fn new(options: ResolveOptions) -> Self {
        Self::new_with_file_system(FileSystemOs, options)
    }
}

impl<Fs: FileSystem> ResolverGeneric<Fs> {
    pub fn new_with_file_system(file_system: Fs, options: ResolveOptions) -> Self {
        let ResolveOptions { version, search_paths, root } = options;
        let scheme = if version.is_luau() {
            Scheme::Luau(Luau::default())
        } else {
            Scheme::Lua(Lua::new(search_paths, root))
        };
        Self { cache: Cache::new(file_system), scheme }
    }

    /// Forgets everything read so far, so the next resolution sees the file
    /// system as it is then. A resolution already running may still answer
    /// from what it read before.
    pub fn clear_cache(&self) {
        self.cache.clear();
        match &self.scheme {
            Scheme::Lua(lua) => lua.clear(),
            Scheme::Luau(luau) => luau.clear(),
        }
    }

    /// The module `module`, a require's string such as `"foo.bar"`,
    /// `"./sibling"`, or `"@alias/module"`, names from `from_file`, the file
    /// requiring it, which only Luau reads.
    ///
    /// # Errors
    ///
    /// Returns why `module` names no module, or more than one.
    pub fn resolve(&self, from_file: &Path, module: &str) -> Result<Resolution, ResolveError> {
        match &self.scheme {
            Scheme::Lua(lua) => {
                let path = lua.resolve(&self.cache, module)?;
                Ok(Resolution { path, is_self_alias_shadowed: false })
            }
            Scheme::Luau(luau) => luau.resolve(&self.cache, from_file, module),
        }
    }
}
