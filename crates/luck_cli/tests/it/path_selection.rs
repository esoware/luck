//! Which files `lint`, `fmt`, and `check` take, and which `luck.json` governs
//! each. These need a working directory, so they spawn the binary.

use std::fs;
use std::path::Path;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, write_file};

const UNINDENTED: &str = "local function f()\nreturn 1\nend\n";

/// A root project indenting with two spaces, and a `pkg` project inside it
/// indenting with tabs.
fn nested_projects() -> TempDir {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "luck.json", r#"{"format":{"indent_style":"spaces","indent_width":2}}"#);
    write_file(root, "pkg/luck.json", r#"{"format":{"indent_style":"tabs"}}"#);
    write_file(root, "src/a.lua", UNINDENTED);
    write_file(root, "pkg/sub/b.lua", UNINDENTED);
    project
}

fn read(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join(name)).expect("read back")
}

/// The paths `fmt --list-different` prints, with `/` separators.
fn listed(root: &Path, args: &[&str]) -> Vec<String> {
    let output =
        luck_in(root).args(["fmt", "--list-different"]).args(args).output().expect("fmt runs");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout)
        .expect("paths are UTF-8")
        .lines()
        .map(|line| line.replace('\\', "/").trim_start_matches("./").to_owned())
        .collect()
}

#[test]
fn a_file_takes_the_nearest_config_above_it() {
    let project = nested_projects();
    luck_in(project.path()).arg("fmt").assert().success();
    assert!(read(project.path(), "src/a.lua").contains("\n  return 1"));
    assert!(read(project.path(), "pkg/sub/b.lua").contains("\n\treturn 1"));
}

#[test]
fn a_named_file_takes_the_nearest_config_whatever_the_working_directory() {
    let project = nested_projects();
    luck_in(project.path()).args(["fmt", "pkg/sub/b.lua"]).assert().success();
    assert!(read(project.path(), "pkg/sub/b.lua").contains("\n\treturn 1"));
}

#[test]
fn stdin_filepath_takes_the_config_nearest_the_path() {
    let project = nested_projects();
    luck_in(project.path())
        .args(["fmt", "--stdin-filepath", "pkg/sub/b.lua"])
        .write_stdin(UNINDENTED)
        .assert()
        .success()
        .stdout(predicate::str::contains("\n\treturn 1"));
}

#[test]
fn config_flag_puts_every_file_under_one_config() {
    let project = nested_projects();
    luck_in(project.path()).args(["fmt", "-c", "luck.json"]).assert().success();
    assert!(read(project.path(), "pkg/sub/b.lua").contains("\n  return 1"));
}

#[test]
fn glob_selects_the_files_it_matches() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "src/a.lua", UNINDENTED);
    write_file(root, "src/deep/b.lua", UNINDENTED);
    write_file(root, "other/c.lua", UNINDENTED);

    assert_eq!(listed(root, &["src/*.lua"]), ["src/a.lua"]);
    assert_eq!(listed(root, &["src/**/*.lua"]), ["src/a.lua", "src/deep/b.lua"]);
    assert_eq!(listed(root, &["./{src,other}/*.lua"]), ["other/c.lua", "src/a.lua"]);
}

#[test]
fn negated_glob_leaves_files_out() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "src/a.lua", UNINDENTED);
    write_file(root, "src/deep/b.lua", UNINDENTED);

    assert_eq!(listed(root, &["!src/deep/**"]), ["src/a.lua"]);
    assert_eq!(listed(root, &["src", "!**/a.lua"]), ["src/deep/b.lua"]);
}

/// A working directory `here` whose config indents with two spaces, beside a
/// `pkg` with no config, which takes the default tabs.
fn sibling_of_a_project() -> TempDir {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "here/luck.json", r#"{"format":{"indent_style":"spaces","indent_width":2}}"#);
    write_file(root, "pkg/a.lua", UNINDENTED);
    project
}

#[test]
fn a_path_through_dot_dot_takes_no_config_from_the_working_directory() {
    for arg in ["../pkg/a.lua", "../pkg", "../pkg/*.lua"] {
        let project = sibling_of_a_project();
        luck_in(&project.path().join("here")).args(["fmt", arg]).assert().success();
        assert!(read(project.path(), "pkg/a.lua").contains("\n\treturn 1"), "{arg}");
    }
}

#[test]
fn glob_outside_the_working_directory_selects_files() {
    let project = sibling_of_a_project();
    let root = project.path();
    let here = root.join("here");
    assert_eq!(listed(&here, &["../pkg/*.lua"]), ["../pkg/a.lua"]);

    let absolute_glob = root.join("pkg").join("*.lua").display().to_string();
    let absolute_file = root.join("pkg").join("a.lua").display().to_string().replace('\\', "/");
    assert_eq!(listed(&here, &[&absolute_glob]), [absolute_file]);
    let dropped = format!("!{absolute_glob}");
    let everything_dropped = [absolute_glob.as_str(), &dropped, "--no-error-on-unmatched-pattern"];
    assert_eq!(listed(&here, &everything_dropped), Vec::<String>::new());
}

#[test]
fn glob_that_matches_nothing_is_an_error_beside_one_that_matches() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "src/a.lua", UNINDENTED);

    luck_in(root)
        .args(["fmt", "--check", "src", "srcc/**/*.lua"])
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("no `.lua` or `.luau` files matched `srcc/**/*.lua`"));
    assert_eq!(
        listed(root, &["src", "srcc/**/*.lua", "--no-error-on-unmatched-pattern"]),
        ["src/a.lua"]
    );
}

#[test]
fn negated_directory_leaves_out_everything_under_it() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "src/a.lua", UNINDENTED);
    write_file(root, "src/deep/b.lua", UNINDENTED);
    write_file(root, "vendor/lib/c.lua", UNINDENTED);

    assert_eq!(listed(root, &["!vendor"]), ["src/a.lua", "src/deep/b.lua"]);
    assert_eq!(listed(root, &["!src/*"]), ["vendor/lib/c.lua"]);
    assert_eq!(listed(root, &["!src/a.lua"]), ["src/deep/b.lua", "vendor/lib/c.lua"]);
}

#[test]
fn glob_still_honors_the_project_exclude() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "luck.json", r#"{"exclude":["src/gen/**"]}"#);
    write_file(root, "src/a.lua", UNINDENTED);
    write_file(root, "src/gen/b.lua", UNINDENTED);

    assert_eq!(listed(root, &["src/**/*.lua"]), ["src/a.lua"]);
}

#[test]
fn named_file_the_config_excludes_is_left_out() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "luck.json", r#"{"exclude":["vendor/**"]}"#);
    write_file(root, "src/a.lua", UNINDENTED);
    write_file(root, "vendor/lib.lua", UNINDENTED);

    assert_eq!(listed(root, &["src/a.lua", "vendor/lib.lua"]), ["src/a.lua"]);
    luck_in(root)
        .args(["fmt", "vendor/lib.lua", "--no-error-on-unmatched-pattern"])
        .assert()
        .success();
    assert_eq!(read(root, "vendor/lib.lua"), UNINDENTED);
}

#[test]
fn no_matching_file_is_a_usage_error() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "notes.txt", "nothing to see\n");

    for command in ["lint", "fmt", "check"] {
        luck_in(root)
            .arg(command)
            .assert()
            .code(2i32)
            .stderr(predicate::str::contains("no `.lua` or `.luau` files matched `.`"));
        luck_in(root)
            .args([command, "src/*.lua"])
            .assert()
            .code(2i32)
            .stderr(predicate::str::contains("matched `src/*.lua`"));
        luck_in(root).args([command, "--no-error-on-unmatched-pattern"]).assert().code(0i32);
    }
}

#[test]
fn missing_path_without_a_wildcard_is_not_a_glob() {
    let project = TempDir::new().expect("project");
    luck_in(project.path())
        .args(["check", "missing.lua"])
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("path not found: `missing.lua`"));
}
