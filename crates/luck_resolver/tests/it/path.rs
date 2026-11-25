use std::path::Path;

use luck_resolver::{lexically_normalized, to_slash_lossy};

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
fn lexically_normalized_resolves_dots_without_reading_the_disk() {
    let working_directory = Path::new("/work/project");
    assert_eq!(
        lexically_normalized(&working_directory.join("./a/../b/x.lua")),
        Path::new("/work/project/b/x.lua")
    );
    assert_eq!(lexically_normalized(&working_directory.join("../other")), Path::new("/work/other"));
}
