//! What the entries of `definitions` take: each path, directory, and glob
//! resolved against the directory of the file that names it, and the files
//! a project's entries and an override's load, in order.
//!
//! A glob reads a tree as TypeScript's `include` does: each directory's files,
//! sorted by name, before its subdirectories, sorted, a symlinked directory
//! followed but each directory entered once, and a wildcard taking no name
//! that starts with `.` unless the pattern spells one.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::slice;
use std::sync::OnceLock;

use globset::GlobMatcher;
use luck_resolver::{FileSystem, is_definition_file};
use luck_stdlib::{SecurityLevel, Stdlib};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::config::DefinitionsEntry;
use crate::glob::Glob;

/// What an entry of a project's `definitions` loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionSource {
    /// Roblox's API, which `"@roblox"` names, at the file's security level.
    Roblox,
    /// A definition file, by its canonical path, with its text.
    File { path: PathBuf, text: String },
}

/// What a file runs with over Luau's library: the definitions it loads, in
/// order, and the security level it reaches Roblox's API at.
#[derive(Debug, Default)]
pub struct Definitions {
    sources: Vec<DefinitionSource>,
    security: SecurityLevel,
    /// Built the first time it's asked for, since only the commands that
    /// analyze code read it.
    stdlib: OnceLock<Stdlib>,
}

impl PartialEq for Definitions {
    fn eq(&self, other: &Self) -> bool {
        self.security == other.security && self.sources == other.sources
    }
}

impl Eq for Definitions {}

impl Definitions {
    /// What `entries` load, in order, each file's text from `texts`, with
    /// `security` when they load `@roblox`.
    ///
    /// Naming a file or `@roblox` puts it at that entry, a glob or directory
    /// adds each file it takes that no entry before it took, and a dropping
    /// entry removes the files it takes.
    pub(crate) fn new<'entry>(
        entries: impl IntoIterator<Item = &'entry DefinitionsEntry>,
        security: Option<SecurityLevel>,
        texts: &FxHashMap<PathBuf, String>,
    ) -> Self {
        let mut loaded: Vec<Loaded<'entry>> = Vec::new();
        let mut taken: FxHashSet<&Path> = FxHashSet::default();
        for entry in entries {
            match entry {
                DefinitionsEntry::BuiltIn(_) => {
                    loaded.retain(|known| *known != Loaded::Roblox);
                    loaded.push(Loaded::Roblox);
                }
                DefinitionsEntry::Files(DefinitionFiles::File(path)) => {
                    if !taken.insert(path) {
                        loaded.retain(|known| *known != Loaded::File(path));
                    }
                    loaded.push(Loaded::File(path));
                }
                DefinitionsEntry::Files(DefinitionFiles::Glob(glob)) => {
                    for path in &glob.files {
                        if taken.insert(path) {
                            loaded.push(Loaded::File(path));
                        }
                    }
                }
                DefinitionsEntry::Dropped(files) => {
                    let dropped: FxHashSet<&Path> =
                        files.paths().iter().map(PathBuf::as_path).collect();
                    if !dropped.is_empty() {
                        loaded.retain(|known| match known {
                            Loaded::File(path) => !dropped.contains(path),
                            Loaded::Roblox => true,
                        });
                        taken.retain(|path| !dropped.contains(path));
                    }
                }
                DefinitionsEntry::Files(DefinitionFiles::Written(_)) => {}
            }
        }
        let sources: Vec<DefinitionSource> = loaded
            .into_iter()
            .map(|source| match source {
                Loaded::Roblox => DefinitionSource::Roblox,
                Loaded::File(path) => DefinitionSource::File {
                    path: path.to_path_buf(),
                    text: texts.get(path).cloned().unwrap_or_default(),
                },
            })
            .collect();
        let has_roblox = sources.contains(&DefinitionSource::Roblox);
        Self {
            security: security.filter(|_| has_roblox).unwrap_or_default(),
            sources,
            stdlib: OnceLock::new(),
        }
    }

    /// What each definition loads, in order.
    pub fn sources(&self) -> &[DefinitionSource] {
        &self.sources
    }

    /// The security level its files reach Roblox's API at, `None` unless
    /// they load `@roblox` at another.
    pub const fn security(&self) -> SecurityLevel {
        self.security
    }

    /// Luau's standard library with what [`Self::sources`] load layered over
    /// it, in order, but for a definition file that doesn't parse.
    pub fn stdlib(&self) -> &Stdlib {
        self.stdlib.get_or_init(|| {
            self.sources.iter().fold(Stdlib::luau(), |below, source| match source {
                DefinitionSource::Roblox => below.with_roblox(self.security),
                DefinitionSource::File { text, .. } => below.layer(text).unwrap_or(below),
            })
        })
    }
}

/// What the order of a file's definitions holds before it reads their texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Loaded<'path> {
    Roblox,
    File(&'path Path),
}

/// The definition files a path entry of `definitions` takes, or after `!`,
/// drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefinitionFiles {
    /// As written, before loading resolves it.
    Written(PathBuf),
    /// A definition file, by its canonical path.
    File(PathBuf),
    /// What a directory or a glob takes.
    Glob(Box<DefinitionGlob>),
}

impl DefinitionFiles {
    /// The canonical path of each definition file it takes, in order.
    pub(crate) fn paths(&self) -> &[PathBuf] {
        match self {
            Self::Written(_) => &[],
            Self::File(path) => slice::from_ref(path),
            Self::Glob(glob) => &glob.files,
        }
    }
}

/// A directory, or a glob below the directory its literal part names, with
/// the definition files it took when the project loaded.
#[derive(Debug, Clone)]
pub(crate) struct DefinitionGlob {
    /// Canonical, when it exists.
    base: PathBuf,
    /// `None` for a directory, which takes every definition file below it.
    pattern: Option<Pattern>,
    files: Vec<PathBuf>,
}

impl PartialEq for DefinitionGlob {
    fn eq(&self, other: &Self) -> bool {
        self.base == other.base
            && self.pattern.as_ref().map(|pattern| &pattern.glob)
                == other.pattern.as_ref().map(|pattern| &pattern.glob)
    }
}

impl Eq for DefinitionGlob {}

impl DefinitionGlob {
    /// Whether it takes a definition file at `located`, a path
    /// [`located_path`](crate::located_path) gives, which needn't exist.
    pub(crate) fn takes(&self, located: &Path) -> bool {
        let Ok(below) = located.strip_prefix(&self.base) else {
            return false;
        };
        let reaches = below.components().all(|component| match component {
            Component::Normal(name) => self.reaches(name),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => false,
        });
        !below.as_os_str().is_empty()
            && is_definition_file(below)
            && reaches
            && self.pattern.as_ref().is_none_or(|pattern| pattern.matcher.is_match(below))
    }

    fn reaches(&self, name: &OsStr) -> bool {
        match &self.pattern {
            Some(pattern) => pattern.reaches(name),
            None => !is_hidden(name),
        }
    }
}

/// The part of a glob below its literal directories.
#[derive(Debug, Clone)]
struct Pattern {
    glob: Glob,
    matcher: GlobMatcher,
    /// Each part without a wildcard, which a name that starts with `.` has to
    /// be one of for a wildcard to reach it, unless `has_dot_wildcard`.
    literal_parts: Vec<String>,
    /// Whether a part that starts with `.` has a wildcard, as `.*` does.
    has_dot_wildcard: bool,
    /// How many levels below the base a match can sit, when no part is `**`
    /// or holds a `{` group, which can hold a `/`.
    max_depth: Option<usize>,
}

impl Pattern {
    fn new(parts: &[String]) -> Result<Self, String> {
        let glob = Glob::new(&parts.join("/"))?;
        let is_unbounded = parts.iter().any(|part| part.contains("**") || part.contains('{'));
        Ok(Self {
            matcher: glob.matcher(),
            glob,
            literal_parts: parts
                .iter()
                .filter(|part| !is_wildcard(OsStr::new(part)))
                .cloned()
                .collect(),
            has_dot_wildcard: parts
                .iter()
                .any(|part| part.starts_with('.') && is_wildcard(OsStr::new(part))),
            max_depth: (!is_unbounded).then_some(parts.len()),
        })
    }

    fn reaches(&self, name: &OsStr) -> bool {
        !is_hidden(name)
            || self.has_dot_wildcard
            || self.literal_parts.iter().any(|part| OsStr::new(part) == name)
    }
}

/// What `written`, a path, directory, or glob a config in `directory` names
/// as the entry `named`, takes, or after `!` when `is_dropped`, drops.
///
/// The literal directories before the first part with a wildcard, or with a
/// `[` or `{`, are the base, and so is a part with one that exists as
/// written. A literal path that doesn't exist is an error, but for one that
/// drops, and a glob may take nothing.
///
/// # Errors
///
/// Returns the problem, naming `named`, when a literal path can't be read or
/// names a file that isn't a definition file, when a `..` follows a
/// wildcard, or when the glob is invalid.
pub(crate) fn resolve(
    file_system: &dyn FileSystem,
    directory: &Path,
    written: &Path,
    named: &str,
    is_dropped: bool,
) -> Result<DefinitionFiles, String> {
    if is_dropped && written.as_os_str().as_encoded_bytes().starts_with(b"@") {
        return Err(format!(
            "`definitions` names `{named}`, but `!` drops only definition files, not a \
             definition set built into luck"
        ));
    }
    let mut base = directory.to_path_buf();
    let mut components = written.components();
    let mut parts = Vec::new();
    for component in components.by_ref() {
        match component {
            Component::Normal(part)
                if is_wildcard(part) && file_system.metadata(&base.join(part)).is_err() =>
            {
                parts.push(part.to_string_lossy().into_owned());
                break;
            }
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir
            | Component::Normal(_) => base.push(component),
        }
    }
    if parts.is_empty() {
        return literal(file_system, &base, written, named, is_dropped);
    }
    for component in components {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                return Err(format!(
                    "`definitions` names `{named}`, but a `..` can't follow a wildcard"
                ));
            }
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
        }
    }
    let pattern = Pattern::new(&parts)?;
    let Ok(canonical) = file_system.canonicalize(&base) else {
        let base = file_system.canonicalize_existing(&base);
        return Ok(DefinitionFiles::Glob(Box::new(DefinitionGlob {
            base,
            pattern: Some(pattern),
            files: Vec::new(),
        })));
    };
    let files = walk(file_system, &canonical, Some(&pattern)).map_err(|e| unreadable(named, &e))?;
    Ok(DefinitionFiles::Glob(Box::new(DefinitionGlob {
        base: canonical,
        pattern: Some(pattern),
        files,
    })))
}

/// What `path`, which `named` writes as `written` with no wildcard, takes.
///
/// A name with no `.` that isn't there is taken for a directory, as
/// TypeScript's `include` takes one, and one with a `.` for a file.
fn literal(
    file_system: &dyn FileSystem,
    path: &Path,
    written: &Path,
    named: &str,
    is_dropped: bool,
) -> Result<DefinitionFiles, String> {
    let metadata = file_system.metadata(path);
    if metadata.as_ref().is_ok_and(|metadata| metadata.is_dir) {
        let base = file_system.canonicalize(path).map_err(|e| unreadable(named, &e))?;
        let files = walk(file_system, &base, None).map_err(|e| unreadable(named, &e))?;
        return Ok(DefinitionFiles::Glob(Box::new(DefinitionGlob { base, pattern: None, files })));
    }
    let is_directory_name =
        written.file_name().is_none_or(|name| !name.as_encoded_bytes().contains(&b'.'));
    if !is_definition_file(written) && (metadata.is_ok() || !is_directory_name) {
        return Err(format!(
            "`definitions` names `{named}`, but a definition file's name ends in `.d.luau`"
        ));
    }
    match metadata {
        Ok(_) => file_system
            .canonicalize(path)
            .map(DefinitionFiles::File)
            .map_err(|e| unreadable(named, &e)),
        Err(_) if is_dropped => Ok(DefinitionFiles::File(file_system.canonicalize_existing(path))),
        Err(e) => Err(unreadable(named, &e)),
    }
}

fn unreadable(named: &str, e: &io::Error) -> String {
    format!("`definitions` names `{named}`, which can't be read: {e}")
}

/// The canonical path of each definition file below `base`, a canonical
/// directory, that `pattern` takes, or with none, every one a wildcard
/// reaches, in the order they load.
fn walk(
    file_system: &dyn FileSystem,
    base: &Path,
    pattern: Option<&Pattern>,
) -> io::Result<Vec<PathBuf>> {
    let mut walk = Walk {
        file_system,
        pattern,
        visited: FxHashSet::default(),
        taken: FxHashSet::default(),
        files: Vec::new(),
    };
    let entries = walk.entries(base)?;
    walk.visited.insert(base.to_path_buf());
    walk.take(base, Path::new(""), 0, entries);
    Ok(walk.files)
}

struct Walk<'walk> {
    file_system: &'walk dyn FileSystem,
    pattern: Option<&'walk Pattern>,
    /// The canonical path of each directory entered, so a symlink back up the
    /// tree ends.
    visited: FxHashSet<PathBuf>,
    taken: FxHashSet<PathBuf>,
    files: Vec<PathBuf>,
}

/// A directory's file names and subdirectory names, each sorted.
type Entries = (Vec<OsString>, Vec<OsString>);

impl Walk<'_> {
    /// The files and subdirectories of `directory`, a symlink sorted as what
    /// it leads to and one that leads nowhere left out.
    fn entries(&self, directory: &Path) -> io::Result<Entries> {
        let (mut files, mut directories) = (Vec::new(), Vec::new());
        for entry in self.file_system.read_dir(directory)? {
            let metadata = entry
                .metadata
                .or_else(|| self.file_system.metadata(&directory.join(&entry.name)).ok());
            match metadata {
                Some(metadata) if metadata.is_file => files.push(entry.name),
                Some(metadata) if metadata.is_dir => directories.push(entry.name),
                Some(_) | None => {}
            }
        }
        files.sort_unstable();
        directories.sort_unstable();
        Ok((files, directories))
    }

    /// Takes the definition files of `directory`, `relative` below the base
    /// and `depth` levels down, which hold `entries`, then those of each
    /// subdirectory.
    fn take(&mut self, directory: &Path, relative: &Path, depth: usize, entries: Entries) {
        let (files, directories) = entries;
        for name in files {
            let below = relative.join(&name);
            let is_match = self.pattern.is_none_or(|pattern| pattern.matcher.is_match(&below));
            if !is_definition_file(&below) || !self.reaches(&name) || !is_match {
                continue;
            }
            if let Ok(canonical) = self.file_system.canonicalize(&directory.join(&name))
                && self.taken.insert(canonical.clone())
            {
                self.files.push(canonical);
            }
        }
        if self.pattern.and_then(|pattern| pattern.max_depth).is_some_and(|max| depth + 1 >= max) {
            return;
        }
        for name in directories {
            if !self.reaches(&name) {
                continue;
            }
            let subdirectory = directory.join(&name);
            let Ok(canonical) = self.file_system.canonicalize(&subdirectory) else {
                continue;
            };
            if !self.visited.insert(canonical) {
                continue;
            }
            // One that can't be read takes nothing, as in TypeScript.
            let Ok(below) = self.entries(&subdirectory) else {
                continue;
            };
            self.take(&subdirectory, &relative.join(&name), depth + 1, below);
        }
    }

    fn reaches(&self, name: &OsStr) -> bool {
        self.pattern.map_or_else(|| !is_hidden(name), |pattern| pattern.reaches(name))
    }
}

fn is_wildcard(part: &OsStr) -> bool {
    part.as_encoded_bytes().iter().any(|b| matches!(b, b'*' | b'?' | b'[' | b'{'))
}

fn is_hidden(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_library_is_built_the_first_time_it_is_asked_for() {
        let entries = [DefinitionsEntry::BuiltIn("roblox".to_owned())];
        let definitions = Definitions::new(&entries, None, &FxHashMap::default());
        assert!(definitions.stdlib.get().is_none());
        assert!(definitions.stdlib().library().has_roblox);
    }
}
