use std::path::{Path, PathBuf};

use luck_config::located_path;
use luck_resolver::FileSystemOs;

use crate::common::{load_error, loaded, project_with};

/// `relative` under `directory` as an editor's document is matched, under
/// canonical directories, as a temporary directory's path on macOS, behind
/// `/var`, or on Windows, by a short name, isn't.
fn document(directory: &Path, relative: &str) -> PathBuf {
    located_path(&FileSystemOs, &directory.join(relative))
}

#[test]
fn star_in_include_stays_inside_one_directory() {
    let (directory, project) = loaded(&[
        ("luck.json", r#"{ "include": ["src/*.luau"] }"#),
        ("src/a.luau", ""),
        ("src/nested/b.luau", ""),
    ]);
    let filter = project.filter();
    assert!(filter.is_included(&directory.path().join("src/a.luau")));
    assert!(
        !filter.is_included(&directory.path().join("src/nested/b.luau")),
        "`*` must not cross `/`; the default include spells out `**/` because of that"
    );
}

#[test]
fn a_file_not_yet_on_disk_matches_relative_to_the_root() {
    let (directory, project) =
        loaded(&[("luck.json", r#"{ "include": ["./src/**/*.luau"] }"#), ("src/a.luau", "")]);
    assert!(
        project.filter().is_included(&directory.path().join("src/unsaved.luau")),
        "globs match relative to the config directory, even for a file not on disk yet"
    );
}

/// An editor's model is Luau because the editor opened it as Luau, so only
/// an `include` it doesn't match leaves it out.
#[test]
fn a_document_with_no_luau_name_is_included_unless_a_glob_leaves_it_out() {
    let (directory, project) = loaded(&[("luck.json", r#"{ "exclude": ["gen/**"] }"#)]);
    let model = document(directory.path(), "model/1");
    assert!(project.filter().includes_document(&model));
    assert!(!project.filter().is_included(&model), "a walk takes `.luau` and `.lua` files");
    assert!(!project.filter().includes_document(&document(directory.path(), "gen/1")));
}

#[test]
fn a_document_an_include_does_not_match_is_left_out() {
    let (directory, project) = loaded(&[("luck.json", r#"{ "include": ["src/**"] }"#)]);
    assert!(!project.filter().includes_document(&document(directory.path(), "model/1")));
    assert!(project.filter().includes_document(&document(directory.path(), "src/1")));
}

#[test]
fn a_bad_glob_is_an_error_on_the_glob() {
    let text = r#"{ "exclude": ["gen/**", "src/{a,b"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert_eq!(diag.code, "E014");
    assert!(diag.message.contains("invalid glob pattern `src/{a,b`"), "{diag:?}");
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""src/{a,b""#[..]));
}

#[test]
fn deeply_nested_glob_is_an_error_instead_of_a_crash() {
    let pattern = format!("{}a{}", "{".repeat(20_000), "}".repeat(20_000));
    let directory = project_with(&[("luck.json", &format!(r#"{{ "include": ["{pattern}"] }}"#))]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert!(diag.message.contains("nest more than"), "{}", diag.message);
}
