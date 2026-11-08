//! How `luck.json` transforms and the per-pass flags layer for `minify` and
//! `bundle`.

use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::luck_in;

#[test]
fn minify_discovers_transforms_from_input_even_with_explicit_target() {
    let project = TempDir::new().expect("project");
    let elsewhere = TempDir::new().expect("cwd");
    fs::write(project.path().join("luck.json"), r#"{"transforms":{"fold_constants":false}}"#)
        .expect("config");
    let source = project.path().join("main.lua");
    fs::write(&source, "return 1 + 2").expect("source");
    luck_in(elsewhere.path())
        .arg("minify")
        .arg(source)
        .args(["-t", "54"])
        .assert()
        .success()
        .stdout("return 1+2");
}

#[test]
fn explicit_config_extends_controls_target_and_transforms() {
    let directory = TempDir::new().expect("directory");
    fs::write(
        directory.path().join("base.json"),
        r#"{"luau":"lua54","transforms":{"fold_constants":false}}"#,
    )
    .expect("base config");
    fs::write(directory.path().join("chosen.json"), r#"{"extends":["base.json"]}"#)
        .expect("config");
    fs::write(directory.path().join("main.luau"), "return 0xA.8 + 1").expect("source");
    luck_in(directory.path())
        .args(["minify", "main.luau", "-c", "chosen.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+1"));
    luck_in(directory.path())
        .args(["minify", "main.luau", "-c", "chosen.json", "-t", "luau"])
        .assert()
        .code(1i32);
}

#[test]
fn cli_disable_overrides_enabled_config_and_stdin_uses_config() {
    let directory = TempDir::new().expect("directory");
    fs::write(directory.path().join("luck.json"), r#"{"transforms":{"fold_constants":true}}"#)
        .expect("config");
    luck_in(directory.path())
        .args(["minify", "-"])
        .write_stdin("return 1 + 2")
        .assert()
        .success()
        .stdout("return 3");
    luck_in(directory.path())
        .args(["minify", "-", "--no-fold-constants"])
        .write_stdin("return 1 + 2")
        .assert()
        .success()
        .stdout("return 1+2");
    fs::write(directory.path().join("luck.json"), r#"{"transforms":{"fold_constants":false}}"#)
        .expect("config");
    luck_in(directory.path())
        .args(["minify", "-"])
        .write_stdin("return 1 + 2")
        .assert()
        .success()
        .stdout("return 1+2");
}

#[test]
fn cli_enable_overrides_disabled_config_and_last_flag_wins() {
    let directory = TempDir::new().expect("directory");
    fs::write(directory.path().join("luck.json"), r#"{"transforms":{"fold_constants":false}}"#)
        .expect("config");
    luck_in(directory.path())
        .args(["minify", "-", "--fold-constants"])
        .write_stdin("return 1 + 2")
        .assert()
        .success()
        .stdout("return 3");
    luck_in(directory.path())
        .args(["minify", "-", "--fold-constants", "--no-fold-constants"])
        .write_stdin("return 1 + 2")
        .assert()
        .success()
        .stdout("return 1+2");
    luck_in(directory.path())
        .args(["minify", "-", "--no-fold-constants", "--fold-constants"])
        .write_stdin("return 1 + 2")
        .assert()
        .success()
        .stdout("return 3");
}

#[test]
fn rename_globals_is_controllable_in_both_directions() {
    let directory = TempDir::new().expect("directory");
    fs::write(directory.path().join("luck.json"), r#"{"transforms":{"rename_globals":true}}"#)
        .expect("config");
    luck_in(directory.path())
        .args(["minify", "-"])
        .write_stdin("someGlobalName = 1\nreturn someGlobalName")
        .assert()
        .success()
        .stdout(predicate::str::contains("someGlobalName").not());
    luck_in(directory.path())
        .args(["minify", "-", "--no-rename-globals"])
        .write_stdin("someGlobalName = 1\nreturn someGlobalName")
        .assert()
        .success()
        .stdout(predicate::str::contains("someGlobalName"));
}

#[test]
fn bundle_minification_uses_discovered_and_explicit_config() {
    let directory = TempDir::new().expect("directory");
    fs::write(directory.path().join("main.lua"), "return 1 + 2").expect("source");
    fs::write(directory.path().join("luck.json"), r#"{"transforms":{"fold_constants":false}}"#)
        .expect("config");
    luck_in(directory.path())
        .args(["bundle", "main.lua", "--minify"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1+2"));
    fs::write(directory.path().join("enabled.json"), r#"{"transforms":{"fold_constants":true}}"#)
        .expect("config");
    luck_in(directory.path())
        .args(["bundle", "main.lua", "--minify", "-c", "enabled.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1+2").not());
    luck_in(directory.path())
        .args(["bundle", "main.lua", "--minify", "-c", "enabled.json", "--no-fold-constants"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1+2"));
}

#[test]
fn invalid_config_is_not_bypassed_by_target_override() {
    let directory = TempDir::new().expect("directory");
    fs::write(directory.path().join("main.lua"), "return 1").expect("source");
    fs::write(directory.path().join("luck.json"), r#"{"transforms":{"fold_constnats":false}}"#)
        .expect("config");
    for operation in ["minify", "bundle"] {
        luck_in(directory.path())
            .args([operation, "main.lua", "-t", "54"])
            .assert()
            .code(2i32)
            .stderr(predicate::str::contains("fold_constnats"));
        luck_in(directory.path())
            .args([operation, "main.lua", "-c", "missing.json"])
            .assert()
            .code(2i32);
    }
}
