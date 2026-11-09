#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_file;
#[cfg(windows)]
use std::os::windows::fs::symlink_file;

use luck_core::ProjectFilter;

use crate::common::project_with;

#[test]
fn bug_star_in_include_stays_inside_one_directory() {
    let project = project_with(&[("src/a.lua", ""), ("src/nested/b.lua", "")]);
    let filter = ProjectFilter::new(project.path(), Some(&["src/*.lua".to_owned()]), None)
        .expect("valid globs");
    assert!(filter.is_included(&project.path().join("src/a.lua")));
    assert!(
        !filter.is_included(&project.path().join("src/nested/b.lua")),
        "`*` must not cross `/`; the default include spells out `**/` because of that"
    );
}

#[test]
fn bug_a_file_not_yet_on_disk_matches_relative_to_the_root() {
    let project = project_with(&[("src/a.lua", "")]);
    let filter = ProjectFilter::new(project.path(), Some(&["src/**/*.lua".to_owned()]), None)
        .expect("valid globs");
    assert!(
        filter.is_included(&project.path().join("src/unsaved.lua")),
        "globs match relative to the config directory, even for a file not on disk yet"
    );
}

#[test]
fn bug_a_symlinked_file_matches_by_its_path_in_the_project() {
    let project = project_with(&[("shared.lua", "return 1"), ("project/src/a.lua", "")]);
    let root = project.path().join("project");
    let link = root.join("src/link.lua");
    // Windows needs Developer Mode or elevation to create a symlink.
    if symlink_file(project.path().join("shared.lua"), &link).is_err() {
        return;
    }
    let filter =
        ProjectFilter::new(&root, Some(&["src/**/*.lua".to_owned()]), None).expect("valid globs");
    assert!(filter.is_included(&link), "`src/link.lua` lives under `src`, wherever its target is");
}

#[test]
fn bug_deeply_nested_glob_is_an_error_instead_of_a_crash() {
    let project = project_with(&[("a.lua", "")]);
    let pattern = format!("{}a{}", "{".repeat(20_000), "}".repeat(20_000));
    // The bug aborts the process before this call returns.
    let filter = ProjectFilter::new(project.path(), Some(&[pattern]), None);
    assert!(filter.is_err(), "a pathologically nested glob must be reported as invalid");
}

#[test]
fn bug_dot_slash_exclude_matches_relative_to_the_root() {
    let project = project_with(&[("gen/a.lua", "")]);
    let filter = ProjectFilter::new(project.path(), None, Some(&["./gen/**".to_owned()]))
        .expect("valid globs");
    assert!(
        !filter.is_included(&project.path().join("gen/a.lua")),
        "`./gen/**` names the same files as `gen/**` and must not let generated code through"
    );
}
