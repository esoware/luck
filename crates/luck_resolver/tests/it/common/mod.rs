use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

use luck_resolver::{DirEntry, FileMetadata, FileSystem, FileSystemOs};
use tempfile::TempDir;

pub(crate) fn project_with(files: &[(&str, &str)]) -> TempDir {
    let directory = TempDir::new().expect("create temp dir");
    write_files(directory.path(), files);
    directory
}

pub(crate) fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (relative_path, contents) in files {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directories");
        }
        fs::write(&path, contents).expect("write file");
    }
}

pub(crate) fn sample_project() -> TempDir {
    project_with(&[
        ("src/main.luau", "-- entry"),
        ("src/utils.luau", "return {}"),
        ("src/helper.lua", "return {}"),
    ])
}

/// The canonical path of `relative_path` in `project`, which must exist.
pub(crate) fn canonical(project: &TempDir, relative_path: &str) -> PathBuf {
    FileSystemOs.canonicalize(&project.path().join(relative_path)).expect("canonicalize")
}

/// Files that exist only in memory, each directory implied by the files
/// under it.
#[derive(Debug)]
pub(crate) struct MemoryFileSystem {
    files: BTreeMap<PathBuf, Vec<u8>>,
}

impl MemoryFileSystem {
    pub(crate) fn new(files: &[(&str, &str)]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(path, contents)| (PathBuf::from(path), contents.as_bytes().to_vec()))
                .collect(),
        }
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files.keys().any(|file| file != path && file.starts_with(path))
    }
}

impl FileSystem for MemoryFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.files.get(path).cloned().ok_or_else(|| io::ErrorKind::NotFound.into())
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        let is_file = self.files.contains_key(path);
        let is_dir = self.is_dir(path);
        if !is_file && !is_dir {
            return Err(io::ErrorKind::NotFound.into());
        }
        Ok(FileMetadata { is_file, is_dir })
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        if !self.is_dir(path) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let names: BTreeSet<&Path> = self
            .files
            .keys()
            .filter_map(|file| file.strip_prefix(path).ok()?.iter().next().map(Path::new))
            .collect();
        Ok(names
            .into_iter()
            .map(|name| DirEntry {
                name: name.as_os_str().to_owned(),
                metadata: self.metadata(&path.join(name)).ok(),
            })
            .collect())
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.metadata(path)?;
        Ok(path.to_path_buf())
    }
}
