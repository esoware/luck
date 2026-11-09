//! How a `-c` path that is a bare file name roots the project.

use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, write_file};

/// A project with a valid `src/main.lua` and an invalid `gen/bad.lua`.
fn project_with_generated_code(config: &str) -> TempDir {
    let project = TempDir::new().expect("project");
    fs::create_dir_all(project.path().join("src")).expect("create src");
    fs::create_dir_all(project.path().join("gen")).expect("create gen");
    write_file(project.path(), "luck.json", config);
    write_file(project.path(), "src/main.lua", "print(1)\n");
    write_file(project.path(), "gen/bad.lua", "local = 1\n");
    project
}

#[test]
fn bare_config_name_applies_a_relative_exclude() {
    let project = project_with_generated_code(r#"{"exclude":["gen/**"]}"#);
    luck_in(project.path())
        .args(["check", "-c", "luck.json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("ok: 1 file(s) checked"));
}

#[test]
fn bare_config_name_applies_a_relative_include() {
    let project = project_with_generated_code(r#"{"include":["src/**/*.lua"]}"#);
    luck_in(project.path())
        .args(["check", "-c", "luck.json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("ok: 1 file(s) checked"));
}

#[test]
fn bare_config_name_loads_a_root_config_with_extends() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "base.json", r#"{"lua":"lua54"}"#);
    write_file(project.path(), "luck.json", r#"{"root":true,"extends":["base.json"]}"#);
    write_file(project.path(), "main.lua", "print(1)\n");
    luck_in(project.path()).args(["check", "-c", "luck.json"]).assert().success();
}
