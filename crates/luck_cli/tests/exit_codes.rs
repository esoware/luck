//! Black-box tests for the documented exit-code contract in `src/cli.rs`:
//! 0 = success, 1 = the operation ran but found problems, 2 = usage/config
//! error (`EXIT_SUCCESS`/`EXIT_FAILURE`/`EXIT_USAGE`). These spawn the real
//! `luck` binary so they exercise `main.rs`'s worker thread and clap's own
//! usage-error exit path, neither of which a unit test can reach.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;
use tempfile::TempDir;

/// A fresh `luck` command rooted in `dir`, so config discovery walks up from a
/// throwaway temp tree rather than the repository the tests run inside.
fn luck_in(dir: &Path) -> Command {
    let mut command = Command::cargo_bin("luck").expect("luck binary builds");
    command.current_dir(dir);
    command
}

fn write_file(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write temp source");
    path
}

#[test]
fn lint_clean_file_exits_success() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "clean.lua", "print(\"hello world\")\n");

    luck_in(dir.path())
        .args(["lint", "clean.lua"])
        .assert()
        .code(0);
}

#[test]
fn lint_undefined_variable_exits_failure() {
    let dir = TempDir::new().unwrap();
    // `undefined_variable` is a default-on Correctness rule with error
    // severity, so reading an unknown global is the exit-1 problem path.
    write_file(dir.path(), "bad.lua", "return missingglobal\n");

    luck_in(dir.path())
        .args(["lint", "bad.lua"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("undefined_variable"));
}

#[test]
fn check_parse_error_exits_failure() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "broken.lua", "local x = 1 +\n");

    luck_in(dir.path())
        .args(["check", "broken.lua"])
        .assert()
        .code(1);
}

#[test]
fn check_valid_file_exits_success() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "ok.lua", "local x = 1\nreturn x\n");

    luck_in(dir.path())
        .args(["check", "ok.lua"])
        .assert()
        .code(0);
}

#[test]
fn fmt_check_unformatted_exits_failure() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "messy.lua", "local    x=1\n");

    luck_in(dir.path())
        .args(["fmt", "--check", "messy.lua"])
        .assert()
        .code(1);
}

#[test]
fn fmt_check_formatted_exits_success() {
    let dir = TempDir::new().unwrap();
    let path = write_file(dir.path(), "tidy.lua", "local    x=1\n");

    // Canonicalize with the formatter itself so the check target is exactly
    // what `luck fmt` produces, keeping the assertion independent of the
    // formatter's specific style choices.
    luck_in(dir.path())
        .args(["fmt", "tidy.lua"])
        .assert()
        .code(0);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("local x = 1")
    );

    luck_in(dir.path())
        .args(["fmt", "--check", "tidy.lua"])
        .assert()
        .code(0);
}

#[test]
fn unknown_subcommand_exits_usage() {
    let dir = TempDir::new().unwrap();

    luck_in(dir.path())
        .arg("frobnicate")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("frobnicate"));
}

#[test]
fn invalid_flag_exits_usage() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "clean.lua", "print(\"hi\")\n");

    luck_in(dir.path())
        .args(["lint", "--nonexistent-flag", "clean.lua"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--nonexistent-flag"));
}

#[test]
fn invalid_config_unknown_key_exits_usage() {
    let dir = TempDir::new().unwrap();
    // `deny_unknown_fields` turns any unrecognized top-level key into a hard
    // config error, which the CLI maps to EXIT_USAGE.
    write_file(dir.path(), "luck.json", "{ \"not_a_real_key\": true }\n");
    write_file(dir.path(), "clean.lua", "print(\"hi\")\n");

    luck_in(dir.path())
        .args(["check", "clean.lua"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Error"));
}

#[test]
fn fmt_range_formats_only_covered_statement() {
    let dir = TempDir::new().unwrap();
    // Two unformatted statements; the range covers only the second line.
    let path = write_file(dir.path(), "partial.lua", "local    a=1\nlocal    b=2\n");

    luck_in(dir.path())
        .args(["fmt", "--range-start", "13", "partial.lua"])
        .assert()
        .code(0);
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("local    a=1"), "{contents}");
    assert!(contents.contains("local b = 2"), "{contents}");
}

#[test]
fn fmt_range_with_multiple_files_exits_usage() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.lua", "local a = 1\n");
    write_file(dir.path(), "b.lua", "local b = 2\n");

    luck_in(dir.path())
        .args(["fmt", "--range-start", "0", "a.lua", "b.lua"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("exactly one input file"));
}

#[test]
fn fmt_range_reversed_bounds_exits_usage() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.lua", "local a = 1\n");

    luck_in(dir.path())
        .args(["fmt", "--range-start", "9", "--range-end", "2", "a.lua"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid format range"));
}
