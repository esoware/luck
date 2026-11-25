//! Which files `lint`, `fmt`, and `check` take, and which `luck.json` governs
//! each. These need a working directory, so they spawn the binary.

use std::fs;
use std::path::Path;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, run, stderr_text, write_file};

const UNINDENTED: &str = "\
local function f()
return 1
end
";

/// A root project indenting with two spaces, and a `pkg` project inside it
/// indenting with tabs.
fn nested_projects() -> TempDir {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "luck.json", r#"{"format":{"indent_style":"spaces","indent_width":2}}"#);
    write_file(root, "pkg/luck.json", r#"{"format":{"indent_style":"tabs"}}"#);
    write_file(root, "src/a.luau", UNINDENTED);
    write_file(root, "pkg/sub/b.luau", UNINDENTED);
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
    assert!(read(project.path(), "src/a.luau").contains("\n  return 1"));
    assert!(read(project.path(), "pkg/sub/b.luau").contains("\n\treturn 1"));
}

#[test]
fn a_named_file_takes_the_nearest_config_whatever_the_working_directory() {
    let project = nested_projects();
    luck_in(project.path()).args(["fmt", "pkg/sub/b.luau"]).assert().success();
    assert!(read(project.path(), "pkg/sub/b.luau").contains("\n\treturn 1"));
}

#[test]
fn stdin_filepath_takes_the_config_nearest_the_path() {
    let project = nested_projects();
    luck_in(project.path())
        .args(["fmt", "--stdin-filepath", "pkg/sub/b.luau"])
        .write_stdin(UNINDENTED)
        .assert()
        .success()
        .stdout(predicate::str::contains("\n\treturn 1"));
}

#[test]
fn config_flag_puts_every_file_under_one_config() {
    let project = nested_projects();
    luck_in(project.path()).args(["fmt", "-c", "luck.json"]).assert().success();
    assert!(read(project.path(), "pkg/sub/b.luau").contains("\n  return 1"));
}

#[test]
fn glob_selects_the_files_it_matches() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "src/a.luau", UNINDENTED);
    write_file(root, "src/deep/b.luau", UNINDENTED);
    write_file(root, "other/c.luau", UNINDENTED);

    assert_eq!(listed(root, &["src/*.luau"]), ["src/a.luau"]);
    assert_eq!(listed(root, &["src/**/*.luau"]), ["src/a.luau", "src/deep/b.luau"]);
    assert_eq!(listed(root, &["./{src,other}/*.luau"]), ["other/c.luau", "src/a.luau"]);
}

#[test]
fn negated_glob_leaves_files_out() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "src/a.luau", UNINDENTED);
    write_file(root, "src/deep/b.luau", UNINDENTED);

    assert_eq!(listed(root, &["!src/deep/**"]), ["src/a.luau"]);
    assert_eq!(listed(root, &["src", "!**/a.luau"]), ["src/deep/b.luau"]);
}

/// A working directory `here` whose config indents with two spaces, beside a
/// `pkg` with no config, which takes the default tabs.
fn sibling_of_a_project() -> TempDir {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "here/luck.json", r#"{"format":{"indent_style":"spaces","indent_width":2}}"#);
    write_file(root, "pkg/a.luau", UNINDENTED);
    project
}

#[test]
fn a_path_through_dot_dot_takes_no_config_from_the_working_directory() {
    for arg in ["../pkg/a.luau", "../pkg", "../pkg/*.luau"] {
        let project = sibling_of_a_project();
        luck_in(&project.path().join("here")).args(["fmt", arg]).assert().success();
        assert!(read(project.path(), "pkg/a.luau").contains("\n\treturn 1"), "{arg}");
    }
}

#[test]
fn glob_outside_the_working_directory_selects_files() {
    let project = sibling_of_a_project();
    let root = project.path();
    let here = root.join("here");
    assert_eq!(listed(&here, &["../pkg/*.luau"]), ["../pkg/a.luau"]);

    let absolute_glob = root.join("pkg").join("*.luau").display().to_string();
    let absolute_file = root.join("pkg").join("a.luau").display().to_string().replace('\\', "/");
    assert_eq!(listed(&here, &[&absolute_glob]), [absolute_file]);
    let dropped = format!("!{absolute_glob}");
    let everything_dropped = [absolute_glob.as_str(), &dropped, "--no-error-on-unmatched-pattern"];
    assert_eq!(listed(&here, &everything_dropped), Vec::<String>::new());
}

#[test]
fn glob_that_matches_nothing_is_an_error_beside_one_that_matches() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "src/a.luau", UNINDENTED);

    luck_in(root)
        .args(["fmt", "--check", "src", "srcc/**/*.luau"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no `.luau` or `.lua` files matched `srcc/**/*.luau`"));
    assert_eq!(
        listed(root, &["src", "srcc/**/*.luau", "--no-error-on-unmatched-pattern"]),
        ["src/a.luau"]
    );
}

#[test]
fn negated_directory_leaves_out_everything_under_it() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "src/a.luau", UNINDENTED);
    write_file(root, "src/deep/b.luau", UNINDENTED);
    write_file(root, "vendor/lib/c.luau", UNINDENTED);

    assert_eq!(listed(root, &["!vendor"]), ["src/a.luau", "src/deep/b.luau"]);
    assert_eq!(listed(root, &["!src/*"]), ["vendor/lib/c.luau"]);
    assert_eq!(listed(root, &["!src/a.luau"]), ["src/deep/b.luau", "vendor/lib/c.luau"]);
}

#[test]
fn glob_still_honors_the_project_exclude() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "luck.json", r#"{"exclude":["src/gen/**"]}"#);
    write_file(root, "src/a.luau", UNINDENTED);
    write_file(root, "src/gen/b.luau", UNINDENTED);

    assert_eq!(listed(root, &["src/**/*.luau"]), ["src/a.luau"]);
}

#[test]
fn named_file_the_config_excludes_is_left_out() {
    let project = TempDir::new().expect("project");
    let root = project.path();
    write_file(root, "luck.json", r#"{"exclude":["vendor/**"]}"#);
    write_file(root, "src/a.luau", UNINDENTED);
    write_file(root, "vendor/lib.luau", UNINDENTED);

    assert_eq!(listed(root, &["src/a.luau", "vendor/lib.luau"]), ["src/a.luau"]);
    luck_in(root)
        .args(["fmt", "vendor/lib.luau", "--no-error-on-unmatched-pattern"])
        .assert()
        .success();
    assert_eq!(read(root, "vendor/lib.luau"), UNINDENTED);
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
            .code(2)
            .stderr(predicate::str::contains("no `.luau` or `.lua` files matched `.`"));
        luck_in(root)
            .args([command, "src/*.luau"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("matched `src/*.luau`"));
        luck_in(root).args([command, "--no-error-on-unmatched-pattern"]).assert().code(0);
    }
}

#[test]
fn missing_path_without_a_wildcard_is_not_a_glob() {
    let project = TempDir::new().expect("project");
    luck_in(project.path())
        .args(["check", "missing.luau"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("path not found: `missing.luau`"));
}

#[test]
fn lint_counts_a_file_reached_twice_once() {
    let directory = TempDir::new().unwrap();
    write_file(
        directory.path(),
        "main.luau",
        "\
local unused = 1
return 1
",
    );

    let output = run(directory.path(), &["lint", ".", "main.luau", "--max-warnings", "1"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "one warning is within `--max-warnings 1` however many arguments reach the file:\n{}",
        stderr_text(&output)
    );
}

#[cfg(windows)]
fn set_directory_listing(directory: &Path, is_allowed: bool) {
    use std::process::Command;

    const EVERYONE: &str = "*S-1-1-0";
    let mut command = Command::new("icacls");
    command.arg(directory);
    if is_allowed {
        command.args(["/remove:d", EVERYONE]);
    } else {
        command.args(["/deny", &format!("{EVERYONE}:(OI)(CI)(RX)")]);
    }
    assert!(command.output().expect("icacls runs").status.success(), "icacls must apply the ACL");
}

#[cfg(unix)]
fn set_directory_listing(directory: &Path, is_allowed: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = if is_allowed { 0o755 } else { 0o000 };
    fs::set_permissions(directory, fs::Permissions::from_mode(mode)).expect("chmod");
}

#[test]
fn check_fails_when_a_directory_cannot_be_walked() {
    let directory = TempDir::new().unwrap();
    let locked = directory.path().join("locked");
    fs::create_dir_all(&locked).unwrap();
    write_file(&locked, "bad.luau", "local = 1\n");
    write_file(directory.path(), "ok.luau", "return 1\n");

    set_directory_listing(&locked, false);
    let output = run(directory.path(), &["check"]);
    set_directory_listing(&locked, true);

    assert!(
        !output.status.success(),
        "a directory that can't be read leaves files unchecked, so the run must fail:\n{}",
        stderr_text(&output)
    );
}
