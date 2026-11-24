use std::fs;

use tempfile::TempDir;

use crate::common::{HELPER_MODULE, TWO_MODULE_ENTRY, run, write_file};

fn two_module_project() -> TempDir {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", TWO_MODULE_ENTRY);
    write_file(directory.path(), "helper.lua", HELPER_MODULE);
    directory
}

#[test]
fn bundle_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.lua", "-o", "main.lua"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.lua")).unwrap(),
        TWO_MODULE_ENTRY,
        "`-o` naming the entry must not destroy the entry's source"
    );
}

#[test]
fn bundle_refuses_to_overwrite_a_dependency() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.lua", "-o", "helper.lua"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("helper.lua")).unwrap(),
        HELPER_MODULE,
        "`-o` naming a bundled module must not destroy the module's source"
    );
}

#[test]
fn bundle_line_map_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.lua", "-o", "bundle.lua", "--line-map", "main.lua"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.lua")).unwrap(),
        TWO_MODULE_ENTRY,
        "`--line-map` naming the entry must not replace its source with JSON"
    );
}

#[test]
fn bundle_rejects_one_path_for_the_output_and_the_line_map() {
    let directory = two_module_project();

    let output =
        run(directory.path(), &["bundle", "main.lua", "-o", "same.lua", "--line-map", "same.lua"]);

    assert!(
        !output.status.success(),
        "one path can't hold both the bundle and its line map, so the run must fail, not lose one"
    );
}

#[test]
fn build_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();
    write_file(directory.path(), "luck.json", r#"{"entry":"main.lua","output":"main.lua"}"#);

    run(directory.path(), &["build", "--quiet"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.lua")).unwrap(),
        TWO_MODULE_ENTRY,
        "an `output` equal to the `entry` must not destroy the entry's source"
    );
}

#[test]
fn build_refuses_to_overwrite_a_dependency() {
    let directory = two_module_project();
    write_file(directory.path(), "luck.json", r#"{"entry":"main.lua","output":"helper.lua"}"#);

    run(directory.path(), &["build", "--quiet"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("helper.lua")).unwrap(),
        HELPER_MODULE,
        "an `output` naming a bundled module must not destroy the module's source"
    );
}

#[test]
fn build_rejects_two_entries_writing_one_output() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "a.lua", "return 1\n");
    write_file(directory.path(), "b.lua", "return 2\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entries":[{"entry":"a.lua","output":"same.lua"},{"entry":"b.lua","output":"same.lua"}]}"#,
    );

    let output = run(directory.path(), &["build", "--quiet"]);

    assert!(
        !output.status.success(),
        "two entries sharing an output lose the first bundle, so the build must fail"
    );
}
