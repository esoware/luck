use std::path::Path;

use luck_resolver::{FileSystem as _, FileSystemOs, to_slash_lossy};

use crate::common::project_with;

#[test]
fn to_slash_lossy_writes_separators_as_forward_slashes() {
    assert_eq!(to_slash_lossy(Path::new("src/main.lua")), "src/main.lua");
    #[cfg(windows)]
    assert_eq!(to_slash_lossy(Path::new(r"C:\src\main.lua")), "C:/src/main.lua");
    // A backslash is part of a name on Unix.
    #[cfg(unix)]
    assert_eq!(to_slash_lossy(Path::new(r"src\main.lua")), r"src\main.lua");
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
