use std::path::Path;

use luck_config::{ProjectConfig, load_or_discover_config, resolve_build_config};

use crate::common::project_with;

fn load_project(project_root: &Path) -> ProjectConfig {
    load_or_discover_config(Some(&project_root.join("luck.json")), project_root)
        .expect("config loads")
        .expect("config was named explicitly")
}

#[test]
fn rejects_an_output_that_overwrites_the_entry() {
    let project = project_with(&[
        ("main.lua", "return 1"),
        ("luck.json", r#"{ "entry": "main.lua", "output_dir": "." }"#),
    ]);
    let builds = resolve_build_config(&load_project(project.path()), None);
    assert!(
        builds.is_err(),
        "a bundle written over its own entry source destroys that source: {builds:?}"
    );
}

#[test]
fn rejects_two_entries_writing_the_same_output() {
    let project = project_with(&[
        ("a.lua", "return 1"),
        ("b.lua", "return 2"),
        (
            "luck.json",
            r#"{ "entries": [
                { "entry": "a.lua", "output": "bundle.lua" },
                { "entry": "b.lua", "output": "bundle.lua" }
            ] }"#,
        ),
    ]);
    let builds = resolve_build_config(&load_project(project.path()), None);
    assert!(builds.is_err(), "the second bundle would silently replace the first one: {builds:?}");
}
