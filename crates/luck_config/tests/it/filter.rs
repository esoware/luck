use luck_config::Project;

use crate::common::{loaded, project_with};

#[test]
fn star_in_include_stays_inside_one_directory() {
    let (directory, project) = loaded(&[
        ("luck.json", r#"{ "include": ["src/*.lua"] }"#),
        ("src/a.lua", ""),
        ("src/nested/b.lua", ""),
    ]);
    let filter = project.filter();
    assert!(filter.is_included(&directory.path().join("src/a.lua")));
    assert!(
        !filter.is_included(&directory.path().join("src/nested/b.lua")),
        "`*` must not cross `/`; the default include spells out `**/` because of that"
    );
}

#[test]
fn a_file_not_yet_on_disk_matches_relative_to_the_root() {
    let (directory, project) =
        loaded(&[("luck.json", r#"{ "include": ["./src/**/*.lua"] }"#), ("src/a.lua", "")]);
    assert!(
        project.filter().is_included(&directory.path().join("src/unsaved.lua")),
        "globs match relative to the config directory, even for a file not on disk yet"
    );
}

#[test]
fn a_bad_glob_is_an_error_on_the_glob() {
    let text = r#"{ "exclude": ["gen/**", "src/{a,b"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let diag = Project::load(&directory.path().join("luck.json")).unwrap_err();
    assert_eq!(diag.code, "E014");
    assert!(diag.message.contains("invalid glob pattern `src/{a,b`"), "{diag:?}");
    assert_eq!(diag.span.text(text), r#""src/{a,b""#);
}

#[test]
fn deeply_nested_glob_is_an_error_instead_of_a_crash() {
    let pattern = format!("{}a{}", "{".repeat(20_000), "}".repeat(20_000));
    let directory = project_with(&[("luck.json", &format!(r#"{{ "include": ["{pattern}"] }}"#))]);
    let diag = Project::load(&directory.path().join("luck.json")).unwrap_err();
    assert!(diag.message.contains("nest more than"), "{}", diag.message);
}
