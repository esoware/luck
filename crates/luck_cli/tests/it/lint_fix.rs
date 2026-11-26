use std::fs;
use std::path::Path;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, run, stderr_text, stdout_text, write_file};

fn make_read_only(path: &Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).expect("set read-only");
}

/// A directory whose `luck.json` loads Roblox's API, where `wait()`, which a
/// suggestion grows into `task.wait()`, is deprecated.
fn roblox_directory() -> TempDir {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", r#"{"definitions":["@roblox"]}"#);
    directory
}

#[test]
fn lint_stdin_fix_json_keeps_findings_out_of_the_fixed_source() {
    let directory = roblox_directory();

    let output = luck_in(directory.path())
        .args(["lint", "--fix-suggestions", "--format", "json", "--stdin-filepath", "buffer.luau"])
        .write_stdin(
            "\
local s = wait()
return s, missingglobal",
        )
        .output()
        .expect("luck runs");

    assert_eq!(
        stdout_text(&output),
        "\
local s = task.wait()
return s, missingglobal",
        "`--fix` over stdin writes the fixed source to stdout, and nothing else may follow it"
    );
}

/// `--fix` over stdin re-lints the fixed text, so spans index that text.
///
/// A fix that grows the file pushes spans past the end of the original, and a
/// renderer panic also exits 1, so the rendered location is the proof.
#[test]
fn lint_stdin_fix_renders_spans_past_the_end_of_the_original() {
    let directory = roblox_directory();

    luck_in(directory.path())
        .args(["lint", "--fix-suggestions", "--stdin-filepath", "buffer.luau"])
        .write_stdin(
            "\
local s = wait()
return s, missingglobal",
        )
        .assert()
        .code(1)
        .stdout(
            "\
local s = task.wait()
return s, missingglobal",
        )
        .stderr(predicate::str::contains("buffer.luau:2:11"));
}

/// A fix that shrinks the file keeps every span in bounds of the original, so
/// rendering against the original misplaces the snippet without any panic.
#[test]
fn lint_stdin_fix_renders_spans_against_the_shortened_text() {
    let directory = TempDir::new().unwrap();

    luck_in(directory.path())
        .args(["lint", "--fix", "--stdin-filepath", "buffer.luau"])
        .args(["-W", "redundant_nil_init"])
        .write_stdin(
            "\
local a = nil
return a, missingglobal
",
        )
        .assert()
        .code(1)
        .stdout(
            "\
local a
return a, missingglobal
",
        )
        .stderr(predicate::str::contains("buffer.luau:2:11"));
}

/// `--fix` leaves a suggestion, which may change what the code does, and
/// `--fix-suggestions` applies it.
#[test]
fn lint_stdin_fix_applies_suggestions_only_when_asked() {
    let directory = TempDir::new().unwrap();
    let source = "\
local unused = 1
return 1";

    luck_in(directory.path())
        .args(["lint", "--fix", "--stdin-filepath", "buffer.luau"])
        .write_stdin(source)
        .assert()
        .code(0)
        .stdout(source);
    luck_in(directory.path())
        .args(["lint", "--fix-suggestions", "--stdin-filepath", "buffer.luau"])
        .write_stdin(source)
        .assert()
        .code(0)
        .stdout(
            "\
local _unused = 1
return 1",
        );
}

/// The outer fix overlaps the inner one, so only a second round can apply it.
#[test]
fn lint_fix_leaves_no_fixable_problem() {
    let directory = TempDir::new().unwrap();
    write_file(
        directory.path(),
        "luck.json",
        r#"{ "lint": { "rules": { "unnecessary_negation": "warn" } } }"#,
    );
    let path = write_file(
        directory.path(),
        "main.luau",
        "\
local a, b, c = 1, 2, 3
return not (a == (not (b == c)))
",
    );

    let output = run(directory.path(), &["lint", "--fix", "main.luau"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr_text(&output));
    assert_eq!(
        fs::read_to_string(&path).expect("main.luau reads"),
        "\
local a, b, c = 1, 2, 3
return a ~= (b ~= c)
"
    );
}

#[test]
fn lint_fix_exits_failure_when_a_file_cannot_be_written() {
    let directory = TempDir::new().unwrap();
    let path = write_file(
        directory.path(),
        "main.luau",
        "\
local t = nil
return t
",
    );
    make_read_only(&path);

    let output = run(directory.path(), &["lint", "--fix", "-W", "redundant_nil_init", "main.luau"]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a file that can't be written fails the run:\n{}",
        stderr_text(&output)
    );
}
