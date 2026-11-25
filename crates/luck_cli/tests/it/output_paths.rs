use std::fs;

use tempfile::TempDir;

use crate::common::{HELPER_MODULE, TWO_MODULE_ENTRY, run, write_file};

fn two_module_project() -> TempDir {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.luau", TWO_MODULE_ENTRY);
    write_file(directory.path(), "helper.luau", HELPER_MODULE);
    directory
}

#[test]
fn bundle_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.luau", "-o", "main.luau"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.luau")).unwrap(),
        TWO_MODULE_ENTRY,
        "`-o` naming the entry must not destroy the entry's source"
    );
}

#[test]
fn bundle_refuses_to_overwrite_a_dependency() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.luau", "-o", "helper.luau"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("helper.luau")).unwrap(),
        HELPER_MODULE,
        "`-o` naming a bundled module must not destroy the module's source"
    );
}

#[test]
fn bundle_line_map_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.luau", "-o", "bundle.luau", "--line-map", "main.luau"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.luau")).unwrap(),
        TWO_MODULE_ENTRY,
        "`--line-map` naming the entry must not replace its source with JSON"
    );
}

#[test]
fn bundle_rejects_one_path_for_the_output_and_the_line_map() {
    let directory = two_module_project();

    let output = run(
        directory.path(),
        &["bundle", "main.luau", "-o", "same.luau", "--line-map", "same.luau"],
    );

    assert!(
        !output.status.success(),
        "one path can't hold both the bundle and its line map, so the run must fail, not lose one"
    );
}

#[test]
fn build_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();
    write_file(directory.path(), "luck.json", r#"{"entry":"main.luau","output":"main.luau"}"#);

    run(directory.path(), &["build", "--quiet"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.luau")).unwrap(),
        TWO_MODULE_ENTRY,
        "an `output` equal to the `entry` must not destroy the entry's source"
    );
}

#[test]
fn build_refuses_to_overwrite_a_dependency() {
    let directory = two_module_project();
    write_file(directory.path(), "luck.json", r#"{"entry":"main.luau","output":"helper.luau"}"#);

    run(directory.path(), &["build", "--quiet"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("helper.luau")).unwrap(),
        HELPER_MODULE,
        "an `output` naming a bundled module must not destroy the module's source"
    );
}

#[test]
fn build_rejects_two_entries_writing_one_output() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "a.luau", "return 1\n");
    write_file(directory.path(), "b.luau", "return 2\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entries":[{"entry":"a.luau","output":"same.luau"},{"entry":"b.luau","output":"same.luau"}]}"#,
    );

    let output = run(directory.path(), &["build", "--quiet"]);

    assert!(
        !output.status.success(),
        "two entries sharing an output lose the first bundle, so the build must fail"
    );
}
