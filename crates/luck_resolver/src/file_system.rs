use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{self, Path, PathBuf};

use crate::path::push_lexically;
#[cfg(windows)]
use crate::path::strip_verbatim_prefix;

/// Everything a resolver reads goes through this, so a caller can resolve
/// against files only it holds, such as an editor's unsaved buffers.
pub trait FileSystem: Send + Sync {
    /// Bytes, since a source need not be UTF-8.
    ///
    /// # Errors
    ///
    /// Returns what stopped `path` from being read, as [`fs::read`] does.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// What `path` is once every link is followed.
    ///
    /// # Errors
    ///
    /// Returns what [`fs::metadata`] does, such as `NotFound`.
    fn metadata(&self, path: &Path) -> io::Result<FileMetadata>;

    /// # Errors
    ///
    /// Returns what stopped the directory from being listed, as
    /// [`fs::read_dir`] does.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;

    /// `path` made absolute, with every link and `..` resolved, each name in
    /// the case the file system stores it, and no Windows verbatim prefix, so
    /// two paths to one file canonicalize alike.
    ///
    /// # Errors
    ///
    /// Returns what [`fs::canonicalize`] does, such as `NotFound`.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;

    /// `path` canonicalized as far as it exists, with the rest resolved
    /// lexically, so a file not on disk yet, or just deleted, compares
    /// against real paths.
    fn canonicalize_existing(&self, path: &Path) -> PathBuf {
        let absolute = path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let mut missing = Vec::new();
        for ancestor in absolute.ancestors() {
            if let Ok(mut canonical) = self.canonicalize(ancestor) {
                push_lexically(&mut canonical, missing.into_iter().rev());
                return canonical;
            }
            missing.extend(ancestor.components().next_back());
        }
        absolute
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMetadata {
    pub is_file: bool,
    pub is_dir: bool,
}

impl From<fs::FileType> for FileMetadata {
    fn from(file_type: fs::FileType) -> Self {
        Self { is_file: file_type.is_file(), is_dir: file_type.is_dir() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: OsString,
    /// What the entry itself is, or `None` when only
    /// [`FileSystem::metadata`] can say, as for a symlink or any other
    /// Windows reparse point.
    pub metadata: Option<FileMetadata>,
}

/// The OS's own file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileSystemOs;

impl FileSystem for FileSystemOs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        Ok(fs::metadata(path)?.file_type().into())
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        fs::read_dir(path)?
            .map(|read_entry| {
                let entry = read_entry?;
                Ok(DirEntry { name: entry.file_name(), metadata: entry_metadata(&entry) })
            })
            .collect()
    }

    #[cfg(windows)]
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path).map(strip_verbatim_prefix)
    }

    #[cfg(not(windows))]
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }
}

fn entry_metadata(entry: &fs::DirEntry) -> Option<FileMetadata> {
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink() || is_reparse_point(entry) {
        return None;
    }
    Some(file_type.into())
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
