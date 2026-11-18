use std::path::PathBuf;

/// Why a require names no module, or more than one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// Every file the module could have been, in the order they were tried:
    /// Lua's as its templates name them, and Luau's from the directory the
    /// search reached.
    NotFound(Vec<PathBuf>),
    /// Luau: the require starts with none of `./`, `../`, and `@`.
    MissingPrefix,
    /// Luau: `@self` with no path after it.
    BareSelfAlias,
    /// Luau: no `.luaurc` in effect declares the alias, given without its
    /// `@`.
    UnknownAlias(String),
    /// Luau: the `.luaurc` at this path doesn't parse, which hides every alias
    /// declared above it.
    MalformedLuaurc(PathBuf),
    /// Luau: the aliases followed until one repeated, lowercased and without
    /// their `@`.
    AliasCycle(Vec<String>),
    /// Luau: the module is both a `.luau` and a `.lua` file.
    BothExtensions { luau: PathBuf, lua: PathBuf },
    /// Luau: the module is both a file and a directory with an `init` file.
    FileAndDirectory { file: PathBuf, init: PathBuf },
    /// Luau: the module, or a directory on the way to it, is both a file and
    /// a directory without an `init` file.
    FileBesideDirectory { file: PathBuf, directory: PathBuf },
}
