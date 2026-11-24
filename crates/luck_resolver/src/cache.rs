use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use rustc_hash::FxHashMap;

use crate::file_system::{FileMetadata, FileSystem};
use crate::shared_cache::SharedCache;

/// What a resolver has read through `fs`, which every thread resolving
/// through it shares.
///
/// It lists each directory a require looks in once, and answers from that
/// listing which files exist and their canonical paths, without a system call
/// per file, since `canonicalize` takes a lock in the OS on Windows. A name
/// the listing can't settle goes to `fs`, such as a symlink or a name that
/// matches an entry only ignoring case.
#[derive(Debug)]
pub(crate) struct Cache<Fs> {
    fs: Fs,
    directories: SharedCache<OsString, Arc<Directory>>,
    /// The canonical paths only `fs` could give.
    canonical_files: SharedCache<OsString, PathBuf>,
}

/// Each fact is found by the first thread that needs it, while any other
/// waits for it.
#[derive(Debug, Default)]
struct Directory {
    /// `None` when the directory can't be listed.
    listing: OnceLock<Option<Listing>>,
    /// `None` when it can't be canonicalized.
    canonical_path: OnceLock<Option<PathBuf>>,
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
    /// `None` for what only the file system can look through, such as a
    /// symlink, and for names that differ only in case.
    metadata: Option<FileMetadata>,
}

/// What a listing says a name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listed {
    Entry(FileMetadata),
    Missing,
}

impl<Fs: FileSystem> Cache<Fs> {
    pub(crate) fn new(fs: Fs) -> Self {
        Self { fs, directories: SharedCache::default(), canonical_files: SharedCache::default() }
    }

    pub(crate) const fn file_system(&self) -> &Fs {
        &self.fs
    }

    pub(crate) fn clear(&self) {
        self.directories.clear();
        self.canonical_files.clear();
    }

    pub(crate) fn is_file(&self, path: &Path) -> bool {
        self.metadata(path).is_some_and(|metadata| metadata.is_file)
    }

    pub(crate) fn is_dir(&self, path: &Path) -> bool {
        self.metadata(path).is_some_and(|metadata| metadata.is_dir)
    }

    /// The canonical path of `file`, which [`Self::is_file`] found.
    pub(crate) fn canonical_file(&self, file: &Path) -> PathBuf {
        if let Some((parent, name)) = split(file) {
            let directory = self.directories.get_or_insert_with(parent.as_os_str(), Arc::default);
            if matches!(
                directory.listed(&self.fs, parent, name),
                Some(Listed::Entry(FileMetadata { is_file: true, .. }))
            ) && let Some(canonical_parent) =
                directory.canonical_path.get_or_init(|| self.fs.canonicalize(parent).ok())
            {
                return canonical_parent.join(name);
            }
        }
        self.canonical_files.get_or_insert_with(file.as_os_str(), || {
            self.fs.canonicalize(file).unwrap_or_else(|_| file.to_path_buf())
        })
    }

    /// What `path` is, or `None` when nothing is there.
    fn metadata(&self, path: &Path) -> Option<FileMetadata> {
        let listed = split(path).and_then(|(parent, name)| {
            let directory = self.directories.get_or_insert_with(parent.as_os_str(), Arc::default);
            directory.listed(&self.fs, parent, name)
        });
        match listed {
            Some(Listed::Entry(metadata)) => Some(metadata),
            Some(Listed::Missing) => None,
            None => self.fs.metadata(path).ok(),
        }
    }
}

impl Directory {
    /// What the listing of this directory, at `path`, says `name` is, or
    /// `None` when only the file system can say.
    fn listed(&self, fs: &impl FileSystem, path: &Path, name: &str) -> Option<Listed> {
        let listing = self.listing.get_or_init(|| read_listing(fs, path).ok()).as_ref()?;
        match listing.entries.get(name.to_ascii_lowercase().as_str()) {
            Some(Entry { name: listed_name, metadata }) if **listed_name == *name => {
                metadata.map(Listed::Entry)
            }
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

/// A name that isn't ASCII is left out, since only the file system can match
/// it.
fn read_listing(fs: &impl FileSystem, path: &Path) -> io::Result<Listing> {
    let mut entries = FxHashMap::default();
    let mut is_ascii = true;
    for entry in fs.read_dir(path)? {
        let Some(name) = entry.name.to_str().filter(|name| name.is_ascii()) else {
            is_ascii = false;
            continue;
        };
        entries
            .entry(name.to_ascii_lowercase().into_boxed_str())
            .and_modify(|listed: &mut Entry| listed.metadata = None)
            .or_insert_with(|| Entry { name: name.into(), metadata: entry.metadata });
    }
    Ok(Listing { entries, is_ascii })
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink as symlink_file;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file;

    use super::*;
    use crate::file_system::FileSystemOs;
    use crate::fixtures::project_with;

    fn assert_answers_as_the_os(cache: &Cache<FileSystemOs>, directory: &Path, names: &[&str]) {
        for name in names {
            let path = directory.join(name);
            assert_eq!(cache.is_file(&path), path.is_file(), "is_file({name:?})");
            assert_eq!(cache.is_dir(&path), path.is_dir(), "is_dir({name:?})");
            if path.is_file() {
                let canonical = FileSystemOs.canonicalize(&path).unwrap();
                assert_eq!(
                    cache.canonical_file(&path).as_os_str(),
                    canonical.as_os_str(),
                    "{name:?}"
                );
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
            &Cache::new(FileSystemOs),
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
            &Cache::new(FileSystemOs),
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
        let cache = Cache::new(FileSystemOs);
        assert_answers_as_the_os(&cache, &project.path().join("src"), &["link.lua", "plain.lua"]);
        assert_eq!(cache.canonical_file(&link), FileSystemOs.canonicalize(&target).unwrap());
    }

    #[test]
    fn a_listing_is_kept_until_the_cache_clears() {
        let project = project_with(&[("src/utils.lua", "")]);
        let utils = project.path().join("src/utils.lua");
        let added = project.path().join("src/added.lua");
        let cache = Cache::new(FileSystemOs);
        let canonical = cache.canonical_file(&utils);
        fs::remove_file(&utils).expect("remove utils");
        fs::write(&added, "").expect("write added");
        assert!(cache.is_file(&utils));
        assert_eq!(cache.canonical_file(&utils), canonical);
        assert!(!cache.is_file(&added));

        cache.clear();
        assert!(!cache.is_file(&utils));
        assert!(cache.is_file(&added));
    }
}
