//! Temporary project trees and requests for the resolver's tests.

use std::fs;
use std::path::Path;

use luck_core::normalize_path;
use luck_resolver::ResolveRequest;
use luck_token::Span;
use tempfile::TempDir;

/// A temporary directory holding `files`, each a `/`-separated path relative
/// to it and that file's contents.
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

/// The Luau tree most tests start from.
pub(crate) fn luau_project() -> TempDir {
    project_with(&[
        ("src/main.luau", "-- entry"),
        ("src/utils.luau", "return {}"),
        ("src/helper.lua", "return {}"),
    ])
}

/// The normalized path of `relative_path` under `project`.
pub(crate) fn normalized(project: &TempDir, relative_path: &str) -> String {
    normalize_path(&project.path().join(relative_path))
}

pub(crate) const fn request<'request>(
    module: &'request str,
    from_file: &'request str,
) -> ResolveRequest<'request> {
    ResolveRequest { module, from_file, span: Span::new(0, 10) }
}
