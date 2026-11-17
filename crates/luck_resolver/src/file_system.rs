use alloc::sync::Arc;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{fs, io};

use rustc_hash::FxHashMap;

use crate::path::{canonical_path, normalize_path};
use crate::shared_cache::SharedCache;

/// The directories a build's requires look in, each listed once and shared by
/// every thread.
///
/// A listing answers for the names in it, a module file's canonical path
/// included, without a system call per file, since `canonicalize` takes a
/// lock in the OS on Windows. A name the listing can't settle goes to the OS,
/// such as a symlink or a name that matches an entry only ignoring case.
#[derive(Debug, Default)]
pub(crate) struct FileSystem {
    directories: SharedCache<PathBuf, Arc<Directory>>,
    /// The canonical paths only the OS could give.
    canonical_files: SharedCache<PathBuf, String>,
}

/// Each fact is found by the first thread that needs it, while any other
/// waits for it.
#[derive(Debug, Default)]
struct Directory {
    /// `None` when the directory can't be listed.
    listing: OnceLock<Option<Listing>>,
    /// `None` when the OS can't canonicalize it.
    canonical_path: OnceLock<Option<String>>,
}

#[derive(Debug)]
struct Listing {
    /// By ASCII-lowercased name.
    entries: FxHashMap<Box<str>, Entry>,
    /// Every name in it is ASCII, so a name that matches none ignoring ASCII
    /// case names nothing, whatever case and normalization rules the file
    /// system follows.
    is_ascii: bool,
}

#[derive(Debug)]
struct Entry {
    name: Box<str>,
    /// `None` for what only the OS can look through, such as a symlink, and
    /// for names that differ only in case.
    kind: Option<Listed>,
}

/// What a listing says a name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listed {
    File,
    Directory,
    Missing,
}

impl FileSystem {
    pub(crate) fn is_file(&self, path: &Path) -> bool {
        self.listed(path).map_or_else(|| path.is_file(), |listed| listed == Listed::File)
    }

    pub(crate) fn is_dir(&self, path: &Path) -> bool {
        self.listed(path).map_or_else(|| path.is_dir(), |listed| listed == Listed::Directory)
    }

    /// The [`normalize_path`] form of `file`, which [`Self::is_file`] found.
    pub(crate) fn canonical_file(&self, file: &Path) -> String {
        if let Some((parent, name)) = split(file) {
            let directory = self.directories.get_or_insert_with(parent, Arc::default);
            if directory.listed(parent, name) == Some(Listed::File)
                && let Some(canonical_parent) =
                    directory.canonical_path.get_or_init(|| canonical_path(parent))
            {
                let separator = if canonical_parent.ends_with('/') { "" } else { "/" };
                return format!("{canonical_parent}{separator}{name}");
            }
        }
        self.canonical_files.get_or_insert_with(file, || normalize_path(file))
    }

    /// What the listing of `path`'s directory says `path` is, or `None` when
    /// only the OS can say.
    fn listed(&self, path: &Path) -> Option<Listed> {
        let (parent, name) = split(path)?;
        self.directories.get_or_insert_with(parent, Arc::default).listed(parent, name)
    }
}

impl Directory {
    /// What the listing of this directory, at `path`, says `name` is.
    fn listed(&self, path: &Path, name: &str) -> Option<Listed> {
        let listing = self.listing.get_or_init(|| read_listing(path).ok()).as_ref()?;
        match listing.entries.get(name.to_ascii_lowercase().as_str()) {
            Some(Entry { name: listed_name, kind }) if **listed_name == *name => *kind,
            Some(_) => None,
            None => listing.is_ascii.then_some(Listed::Missing),
        }
    }
}

/// `path`'s directory and name, when every file system reads the name as
/// written.
///
/// That's an ASCII name with no `~`, which can start a Windows short name, no
/// `:`, which can name a Windows stream, no trailing `.` or space, which
/// Windows drops, and nothing after it, such as a separator or a `.`
/// component.
fn split(path: &Path) -> Option<(&Path, &str)> {
    let name = path.file_name()?.to_str()?;
    let is_plain = name.is_ascii() && !name.contains(['~', ':']) && !name.ends_with(['.', ' ']);
    if !is_plain || !path.as_os_str().as_encoded_bytes().ends_with(name.as_bytes()) {
        return None;
    }
    Some((path.parent()?, name))
}

/// A name that isn't ASCII is left out, since only the OS can match it.
///
/// # Errors
///
/// Returns the error that stopped the directory from being read.
fn read_listing(path: &Path) -> io::Result<Listing> {
    let mut entries = FxHashMap::default();
    let mut is_ascii = true;
    for read_entry in fs::read_dir(path)? {
        let entry = read_entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str().filter(|name| name.is_ascii()) else {
            is_ascii = false;
            continue;
        };
        let file_type = entry.file_type().ok().filter(|_| !is_reparse_point(&entry));
        #[expect(clippy::filetype_is_file, reason = "`Path::is_file` means a regular file")]
        let kind = file_type.and_then(|plain_type| {
            if plain_type.is_file() {
                Some(Listed::File)
            } else if plain_type.is_dir() {
                Some(Listed::Directory)
            } else {
                None
            }
        });
        entries
            .entry(name.to_ascii_lowercase().into_boxed_str())
            .and_modify(|listed: &mut Entry| listed.kind = None)
            .or_insert_with(|| Entry { name: name.into(), kind });
    }
    Ok(Listing { entries, is_ascii })
}

/// Any reparse point, not just the symlinks `FileType` reports, since the OS
/// may open one as something else or not at all. Free on Windows, where a
/// listing carries each entry's attributes.
#[cfg(windows)]
fn is_reparse_point(entry: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    !entry
        .metadata()
        .is_ok_and(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0)
}

#[cfg(not(windows))]
const fn is_reparse_point(_: &fs::DirEntry) -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::symlink as symlink_file;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file;

    use super::*;
    use crate::fixtures::project_with;

    fn assert_answers_as_the_os(files: &FileSystem, directory: &Path, names: &[&str]) {
        for name in names {
            let path = directory.join(name);
            assert_eq!(files.is_file(&path), path.is_file(), "is_file({name:?})");
            assert_eq!(files.is_dir(&path), path.is_dir(), "is_dir({name:?})");
            if path.is_file() {
                assert_eq!(files.canonical_file(&path), normalize_path(&path), "{name:?}");
            }
        }
    }

    #[test]
    fn a_listing_answers_as_the_os_does() {
        let project = project_with(&[
            ("src/utils.lua", ""),
            ("src/Mixed.luau", ""),
            ("src/long_module_name.lua", ""),
            ("src/pkg/init.lua", ""),
            ("src/.luaurc", "{}"),
            // A stream of `utils.lua` on NTFS, and a file of its own elsewhere.
            ("src/utils.lua:stream", ""),
        ]);
        assert_answers_as_the_os(
            &FileSystem::default(),
            &project.path().join("src"),
            &[
                "utils.lua",
                "UTILS.lua",
                "utils.luau",
                "Mixed.luau",
                "mixed.luau",
                "pkg",
                "PKG",
                "pkg/init.lua",
                "pkg/init.luau",
                ".luaurc",
                "missing.lua",
                "missing/init.lua",
                "LONG_M~1.LUA",
                "utils.lua.",
                "utils.lua ",
                "utils.lua/",
                "utils.lua/.",
                "utils.lua:stream",
                "",
                ".",
                "..",
            ],
        );
    }

    #[test]
    fn a_name_beside_one_that_is_not_ascii_goes_to_the_os() {
        let project = project_with(&[("src/\u{e9}.lua", ""), ("src/plain.lua", "")]);
        assert_answers_as_the_os(
            &FileSystem::default(),
            &project.path().join("src"),
            &["\u{e9}.lua", "\u{c9}.lua", "e\u{301}.lua", "e.lua", "plain.lua", "missing.lua"],
        );
    }

    #[test]
    fn a_symlink_goes_to_the_os() {
        let project = project_with(&[("target/module.lua", ""), ("src/plain.lua", "")]);
        let target = project.path().join("target/module.lua");
        let link = project.path().join("src/link.lua");
        // Windows lets only an administrator or developer mode make one.
        if symlink_file(&target, &link).is_err() {
            return;
        }
        let files = FileSystem::default();
        assert_answers_as_the_os(&files, &project.path().join("src"), &["link.lua", "plain.lua"]);
        assert_eq!(files.canonical_file(&link), normalize_path(&target));
    }

    #[test]
    fn a_listing_is_never_reread() {
        let project = project_with(&[("src/utils.lua", "")]);
        let utils = project.path().join("src/utils.lua");
        let files = FileSystem::default();
        let canonical = files.canonical_file(&utils);
        fs::remove_file(&utils).expect("remove utils");
        fs::write(project.path().join("src/added.lua"), "").expect("write added");
        assert!(files.is_file(&utils));
        assert_eq!(files.canonical_file(&utils), canonical);
        assert!(!files.is_file(&project.path().join("src/added.lua")));
    }
}
