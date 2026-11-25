use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::file_system::{FileSystem as _, FileSystemOs};

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

pub(crate) fn canonical(project: &TempDir, relative_path: &str) -> PathBuf {
    FileSystemOs.canonicalize(&project.path().join(relative_path)).expect("canonicalize")
}
