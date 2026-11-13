use luck_config::ProjectFilter;

use crate::common::project_with;

#[test]
fn star_in_include_stays_inside_one_directory() {
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
fn a_file_not_yet_on_disk_matches_relative_to_the_root() {
    let project = project_with(&[("src/a.lua", "")]);
    let filter = ProjectFilter::new(project.path(), Some(&["src/**/*.lua".to_owned()]), None)
        .expect("valid globs");
    assert!(
        filter.is_included(&project.path().join("src/unsaved.lua")),
        "globs match relative to the config directory, even for a file not on disk yet"
    );
}

#[test]
fn deeply_nested_glob_is_an_error_instead_of_a_crash() {
    let project = project_with(&[("a.lua", "")]);
    let pattern = format!("{}a{}", "{".repeat(20_000), "}".repeat(20_000));
    let filter = ProjectFilter::new(project.path(), Some(&[pattern]), None);
    assert!(filter.is_err(), "a pathologically nested glob must be reported as invalid");
}
