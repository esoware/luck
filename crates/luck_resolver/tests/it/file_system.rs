use std::{fs, io};

use luck_resolver::{FileSystem as _, FileSystemOs};
use tempfile::TempDir;

use crate::common::project_with;

#[test]
fn read_reads_bytes_that_are_not_utf8() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("latin1.lua");
    fs::write(&path, b"local x = 1 -- caf\xE9\n").unwrap();
    assert_eq!(FileSystemOs.read(&path).unwrap(), b"local x = 1 -- caf\xE9\n");
}

#[test]
fn read_of_a_missing_file_is_not_found() {
    let directory = TempDir::new().unwrap();
    let missing = FileSystemOs.read(&directory.path().join("does_not_exist.lua")).unwrap_err();
    assert_eq!(missing.kind(), io::ErrorKind::NotFound);
}

#[test]
fn canonicalize_gives_an_absolute_path_without_a_verbatim_prefix() {
    let project = project_with(&[("src/main.lua", "return {}")]);
    let canonical = FileSystemOs
        .canonicalize(&project.path().join("src/../src/main.lua"))
        .expect("canonicalize");
    assert!(canonical.is_absolute(), "{}", canonical.display());
    assert!(
        !canonical.as_os_str().to_string_lossy().starts_with(r"\\?\"),
        "{}",
        canonical.display()
    );
    assert!(canonical.ends_with("src/main.lua"), "{}", canonical.display());
}

#[test]
fn canonicalize_existing_resolves_only_the_part_that_is_missing() {
    let directory = TempDir::new().unwrap();
    let canonical_root = FileSystemOs.canonicalize(directory.path()).unwrap();
    assert_eq!(
        FileSystemOs.canonicalize_existing(&directory.path().join("gone/../out/bundle.lua")),
        canonical_root.join("out").join("bundle.lua")
    );
    assert_eq!(FileSystemOs.canonicalize_existing(directory.path()), canonical_root);
}
