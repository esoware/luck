use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use crate::aliases::{AliasCache, AliasTarget};
use crate::cache::Cache;
use crate::file_system::FileSystem;
use crate::shared_cache::SharedCache;
use crate::{Resolution, ResolveError};

/// Luau's require by string, which resolves a relative or `@alias` require
/// one path component at a time, as Luau's navigator does.
///
/// Many requires pass through the same directories and reach the same
/// modules, so it keeps what each module path holds.
#[derive(Debug, Default)]
pub(crate) struct Luau {
    aliases: AliasCache,
    modules: SharedCache<OsString, ModuleFiles>,
}

#[derive(Debug, Clone, Copy)]
struct ExtensionFiles {
    has_luau: bool,
    has_lua: bool,
}

#[derive(Debug, Clone, Copy)]
struct ModuleFiles {
    module: ExtensionFiles,
    /// The directory's `init` files, or `None` when it isn't a directory.
    init: Option<ExtensionFiles>,
}

impl ModuleFiles {
    fn read(cache: &Cache<impl FileSystem>, module_path: &Path) -> Self {
        // `init` itself can't be named as a file, only reached through its
        // directory.
        let can_be_a_file = module_path.file_name() != Some(OsStr::new("init"));
        Self {
            module: ExtensionFiles {
                has_luau: can_be_a_file && cache.is_file(&append_extension(module_path, ".luau")),
                has_lua: can_be_a_file && cache.is_file(&append_extension(module_path, ".lua")),
            },
            init: cache.is_dir(module_path).then(|| ExtensionFiles {
                has_luau: cache.is_file(&module_path.join("init.luau")),
                has_lua: cache.is_file(&module_path.join("init.lua")),
            }),
        }
    }
}

enum Probed {
    Module(PathBuf),
    /// A directory without an init file, which a path can walk through but
    /// which is no module itself.
    Directory,
    Missing,
}

impl Luau {
    pub(crate) fn clear(&self) {
        self.aliases.clear();
        self.modules.clear();
    }

    #[cfg(test)]
    pub(crate) const fn aliases(&self) -> &AliasCache {
        &self.aliases
    }

    pub(crate) fn resolve(
        &self,
        cache: &Cache<impl FileSystem>,
        from_file: &Path,
        module: &str,
    ) -> Result<Resolution, ResolveError> {
        let require_path = module.replace('\\', "/");
        let alias_path = require_path.strip_prefix('@');
        if alias_path.is_none()
            && !require_path.starts_with("./")
            && !require_path.starts_with("../")
        {
            return Err(ResolveError::MissingPrefix);
        }
        let requiring_module = requiring_module_path(from_file);
        // Luau resets to the requiring module before it navigates, so an
        // ambiguous one fails every require it makes with a valid prefix.
        self.probe(cache, &requiring_module)?;
        let parent = requiring_module.parent().unwrap_or_else(|| Path::new("."));
        if let Some(alias) = alias_path {
            return self.resolve_alias(cache, alias, parent, &requiring_module);
        }
        let path = self.navigate(cache, parent, &require_path)?;
        Ok(Resolution { path, is_self_alias_shadowed: false })
    }

    /// Resolves `@alias` or `@alias/sub/path`, given without its `@`.
    /// `lookup_directory` is the requiring module's parent, where the search
    /// for `.luaurc` files starts.
    fn resolve_alias(
        &self,
        cache: &Cache<impl FileSystem>,
        alias: &str,
        lookup_directory: &Path,
        requiring_module: &Path,
    ) -> Result<Resolution, ResolveError> {
        let (alias_name, sub_path) =
            alias.split_once('/').map_or((alias, None), |(name, rest)| (name, Some(rest)));

        if alias_name.eq_ignore_ascii_case("self") {
            return self.resolve_self(cache, sub_path, lookup_directory, requiring_module);
        }

        let (directory, full_sub_path) =
            self.follow_alias(cache, lookup_directory, alias_name, sub_path.unwrap_or(""))?;
        if matches!(self.probe(cache, &directory)?, Probed::Missing) {
            return Err(not_found(&directory, &full_sub_path));
        }
        let path = self.navigate(cache, &directory, &full_sub_path)?;
        Ok(Resolution { path, is_self_alias_shadowed: false })
    }

    /// Follows `alias_name` through any aliases of aliases to the directory
    /// the chain ends at, with `sub_path` preceded by what the chain added.
    fn follow_alias(
        &self,
        cache: &Cache<impl FileSystem>,
        lookup_directory: &Path,
        alias_name: &str,
        sub_path: &str,
    ) -> Result<(PathBuf, String), ResolveError> {
        let mut directory = lookup_directory.to_path_buf();
        let mut name = alias_name.to_owned();
        let mut full_sub_path = sub_path.to_owned();
        let mut followed: Vec<String> = Vec::new();
        loop {
            let lowercase_name = name.to_ascii_lowercase();
            if followed.contains(&lowercase_name) {
                followed.push(lowercase_name);
                return Err(ResolveError::AliasCycle(followed));
            }
            let aliases = self.aliases.in_effect(cache.file_system(), &directory);
            match aliases.targets.get(&lowercase_name) {
                Some(AliasTarget::Directory(target_directory)) => {
                    return Ok((target_directory.clone(), full_sub_path));
                }
                Some(AliasTarget::Chained { target, declared_in }) => {
                    // Luau finds the next alias in the file that declared this
                    // one or above it, and restarts its cycle check at a new
                    // file.
                    if *declared_in != directory {
                        followed.clear();
                    }
                    let (next_name, added_path) = target.split_once('/').unwrap_or((target, ""));
                    full_sub_path = format!("{added_path}/{full_sub_path}");
                    followed.push(lowercase_name);
                    next_name.clone_into(&mut name);
                    directory.clone_from(declared_in);
                }
                None => {
                    return Err(aliases
                        .malformed_luaurc
                        .clone()
                        .map_or(ResolveError::UnknownAlias(name), ResolveError::MalformedLuaurc));
                }
            }
        }
    }

    /// `@self` walks down from the requiring module itself, so `@self/x` is
    /// `dir/x` from `dir/init.luau` and `main/x` from `main.luau`, never a
    /// sibling. A `.luaurc` alias named `self` can't override it.
    fn resolve_self(
        &self,
        cache: &Cache<impl FileSystem>,
        sub_path: Option<&str>,
        lookup_directory: &Path,
        requiring_module: &Path,
    ) -> Result<Resolution, ResolveError> {
        let relative_path = sub_path.ok_or(ResolveError::BareSelfAlias)?;
        let is_self_alias_shadowed = self
            .aliases
            .in_effect(cache.file_system(), lookup_directory)
            .targets
            .contains_key("self");
        let path = self.navigate(cache, requiring_module, relative_path)?;
        Ok(Resolution { path, is_self_alias_shadowed })
    }

    /// Walks `/`-separated `sub_path` from `start` to the module it ends at,
    /// and returns that module's canonical path. Empty and `.` components are
    /// skipped, and `..` leaves the current directory unchecked, as in Luau.
    fn navigate(
        &self,
        cache: &Cache<impl FileSystem>,
        start: &Path,
        sub_path: &str,
    ) -> Result<PathBuf, ResolveError> {
        let mut position = start.to_path_buf();
        let mut probed_position = None;
        for component in sub_path.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    if !position.pop() {
                        return Err(not_found(start, sub_path));
                    }
                    probed_position = None;
                }
                ".config" => return Err(not_found(start, sub_path)),
                // Windows reads a name such as `C:` as a drive, which `push`
                // would put in place of the whole path.
                child
                    if !matches!(
                        Path::new(child).components().next(),
                        Some(Component::Normal(_))
                    ) =>
                {
                    return Err(not_found(start, sub_path));
                }
                child => {
                    position.push(child);
                    let child_probe = self.probe(cache, &position)?;
                    if matches!(child_probe, Probed::Missing) {
                        return Err(not_found(start, sub_path));
                    }
                    probed_position = Some(child_probe);
                }
            }
        }
        let probed = match probed_position {
            Some(probed) => probed,
            None => self.probe(cache, &position)?,
        };
        match probed {
            Probed::Module(found) => Ok(cache.canonical_file(&found)),
            Probed::Directory | Probed::Missing => Err(not_found(start, sub_path)),
        }
    }

    /// What `module_path`, a path with no extension, names: a `.luau` or
    /// `.lua` file, a directory with or without an `init` file, or nothing.
    fn probe(
        &self,
        cache: &Cache<impl FileSystem>,
        module_path: &Path,
    ) -> Result<Probed, ResolveError> {
        let files = self
            .modules
            .get_or_insert_with(module_path.as_os_str(), || ModuleFiles::read(cache, module_path));
        // Append the extension to the full name: `with_extension` replaces
        // everything after the last dot, so `./config.v2` would probe
        // `config.luau` instead of `config.v2.luau`.
        let module_file =
            existing_one(files.module, |extension| append_extension(module_path, extension))?;
        let Some(init_files) = files.init else {
            return Ok(module_file.map_or(Probed::Missing, Probed::Module));
        };
        let init_file =
            existing_one(init_files, |extension| module_path.join(["init", extension].concat()))?;
        match (module_file, init_file) {
            (Some(file), Some(init)) => Err(ResolveError::FileAndDirectory { file, init }),
            (Some(file), None) => Err(ResolveError::FileBesideDirectory {
                file,
                directory: module_path.to_path_buf(),
            }),
            (None, Some(init)) => Ok(Probed::Module(init)),
            (None, None) => Ok(Probed::Directory),
        }
    }
}

/// The module `from_file` is: `dir` for `dir/init.luau`, and the path without
/// its extension for any other Luau file.
fn requiring_module_path(from_file: &Path) -> PathBuf {
    if matches!(from_file.file_name().and_then(OsStr::to_str), Some("init.luau" | "init.lua")) {
        from_file.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    } else if matches!(from_file.extension().and_then(OsStr::to_str), Some("luau" | "lua")) {
        from_file.with_extension("")
    } else {
        from_file.to_path_buf()
    }
}

/// [`ResolveError::NotFound`] with the four files the whole of `sub_path`
/// could have been.
fn not_found(start: &Path, sub_path: &str) -> ResolveError {
    let trimmed_path = sub_path.trim_start_matches('/');
    let target =
        if trimmed_path.is_empty() { start.to_path_buf() } else { start.join(trimmed_path) };
    ResolveError::NotFound(vec![
        append_extension(&target, ".luau"),
        append_extension(&target, ".lua"),
        target.join("init.luau"),
        target.join("init.lua"),
    ])
}

/// The one of the files `with_extension` makes of `.luau` and `.lua` that
/// exists.
fn existing_one(
    files: ExtensionFiles,
    with_extension: impl Fn(&str) -> PathBuf,
) -> Result<Option<PathBuf>, ResolveError> {
    match (files.has_luau, files.has_lua) {
        (true, true) => Err(ResolveError::BothExtensions {
            luau: with_extension(".luau"),
            lua: with_extension(".lua"),
        }),
        (true, false) => Ok(Some(with_extension(".luau"))),
        (false, true) => Ok(Some(with_extension(".lua"))),
        (false, false) => Ok(None),
    }
}

fn append_extension(base_path: &Path, extension: &str) -> PathBuf {
    let mut name = base_path.as_os_str().to_os_string();
    name.push(extension);
    PathBuf::from(name)
}
