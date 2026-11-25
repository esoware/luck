//! These spawn the real `luck` binary, so they reach `main.rs`'s worker thread
//! and clap's own usage-error exit, which unit tests can't.

use std::fs;
use std::path::Path;
use std::time::Duration;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, run, stderr_text, write_file};

#[test]
fn lint_clean_file_exits_success() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "clean.lua", "print(\"hello world\")\n");

    luck_in(directory.path()).args(["lint", "clean.lua"]).assert().code(0);
}

#[test]
fn lint_undefined_variable_exits_failure() {
    let directory = TempDir::new().unwrap();
    // `undefined_variable` is on by default as an error.
    write_file(directory.path(), "bad.lua", "return missingglobal\n");

    luck_in(directory.path())
        .args(["lint", "bad.lua"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("undefined_variable"));
}

#[test]
fn threads_takes_a_count_of_one_or_more() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "bad.lua", "return missingglobal\n");

    luck_in(directory.path()).args(["--threads", "1", "lint", "bad.lua"]).assert().code(1);
    luck_in(directory.path())
        .args(["lint", "bad.lua", "--threads", "0"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--threads"));
}

#[test]
fn check_parse_error_exits_failure() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "broken.lua", "local x = 1 +\n");

    luck_in(directory.path()).args(["check", "broken.lua"]).assert().code(1);
}

#[test]
fn check_valid_file_exits_success() {
    let directory = TempDir::new().unwrap();
    write_file(
        directory.path(),
        "ok.lua",
        "\
local x = 1
return x
",
    );

    luck_in(directory.path()).args(["check", "ok.lua"]).assert().code(0);
}

#[test]
fn fmt_check_unformatted_exits_failure() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "messy.lua", "local    x=1\n");

    luck_in(directory.path()).args(["fmt", "--check", "messy.lua"]).assert().code(1);
}

#[test]
fn fmt_check_formatted_exits_success() {
    let directory = TempDir::new().unwrap();
    let path = write_file(directory.path(), "tidy.lua", "local    x=1\n");

    // Format with luck first, so the check doesn't depend on the formatter's
    // style choices.
    luck_in(directory.path()).args(["fmt", "tidy.lua"]).assert().code(0);
    assert!(fs::read_to_string(&path).unwrap().contains("local x = 1"));

    luck_in(directory.path()).args(["fmt", "--check", "tidy.lua"]).assert().code(0);
}

#[test]
fn unknown_subcommand_exits_usage() {
    let directory = TempDir::new().unwrap();

    luck_in(directory.path())
        .arg("frobnicate")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("frobnicate"));
}

#[test]
fn invalid_flag_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "clean.lua", "print(\"hi\")\n");

    luck_in(directory.path())
        .args(["lint", "--nonexistent-flag", "clean.lua"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--nonexistent-flag"));
}

#[test]
fn invalid_config_unknown_key_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", "{ \"not_a_real_key\": true }\n");
    write_file(directory.path(), "clean.lua", "print(\"hi\")\n");

    luck_in(directory.path())
        .args(["check", "clean.lua"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("E014"))
        .stderr(predicate::str::contains("unknown field `not_a_real_key`"))
        .stderr(predicate::str::contains("luck.json:1:3"));
}

/// Every config loads before the parallel section, because a worker reporting
/// an error would deadlock on the stdio locks the main thread holds there.
///
/// The timeout turns that hang into a failure.
fn write_bad_dialect_project(directory: &Path) {
    write_file(directory, "luck.json", "{ \"lua\": \"lua99\" }\n");
    for name in ["a.lua", "b.lua", "c.lua", "d.lua"] {
        write_file(directory, name, "return 1\n");
    }
}

#[test]
fn fmt_bad_dialect_with_multiple_files_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_bad_dialect_project(directory.path());

    luck_in(directory.path())
        .args(["fmt", "--check"])
        .timeout(Duration::from_secs(60))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown variant `lua99`"));
}

#[test]
fn lint_bad_dialect_with_multiple_files_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_bad_dialect_project(directory.path());

    luck_in(directory.path())
        .arg("lint")
        .timeout(Duration::from_secs(60))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown variant `lua99`"));
}

#[test]
fn check_bad_dialect_with_multiple_files_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_bad_dialect_project(directory.path());

    luck_in(directory.path())
        .arg("check")
        .timeout(Duration::from_secs(60))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown variant `lua99`"));
}

/// An empty `entries` list is a config error, not a panic or a silent exit 0.
#[test]
fn build_empty_entries_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", "{ \"entries\": [] }\n");

    luck_in(directory.path())
        .arg("build")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("`entries` is empty"));

    luck_in(directory.path())
        .args(["build", "--quiet"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("`entries` is empty"));
}

#[test]
fn build_rejects_release_with_dev() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "return 1\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entry":"main.lua","output":"bundle.lua","profiles":{"dev":{},"release":{}}}"#,
    );

    let output = run(directory.path(), &["build", "--release", "--dev", "--dry-run"]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "the shorthands select contradictory profiles, as `--profile` given twice does"
    );
}

#[test]
fn minify_missing_input_exits_usage() {
    let directory = TempDir::new().unwrap();

    let output = run(directory.path(), &["minify", "missing.lua"]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "a path that doesn't exist exits 2, as it does for every other command:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn bad_target_flag_is_reported_before_the_config() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", "{ \"not_a_real_key\": true }\n");
    write_file(directory.path(), "main.lua", "return 1\n");

    for command in ["bundle", "minify", "graph"] {
        luck_in(directory.path())
            .args([command, "main.lua", "-t", "lua99"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("invalid value 'lua99' for '--target <TARGET>'"))
            .stderr(predicate::str::contains("invalid target `lua99`"))
            .stderr(predicate::str::contains("not_a_real_key").not());
    }
}

#[test]
fn bad_dynamic_require_flag_is_reported_before_the_config() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", "{ \"not_a_real_key\": true }\n");
    write_file(directory.path(), "main.lua", "return 1\n");

    for command in ["bundle", "graph"] {
        luck_in(directory.path())
            .args([command, "main.lua", "--dynamic-require", "maybe"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "invalid value 'maybe' for '--dynamic-require <MODE>'",
            ))
            .stderr(predicate::str::contains("not_a_real_key").not());
    }
}

/// A `require(expr)` warns and emits a bundle that resolves the call at runtime,
/// and only fails when the project (or the flag) asks for strictness.
#[test]
fn bundle_with_dynamic_require_succeeds_and_warns() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "helper.lua", "return { value = 1 }\n");
    write_file(
        directory.path(),
        "main.lua",
        "\
local helper = require(\"helper\")
local name = \"helper\"
return helper.value + require(name).value
",
    );

    let assert = luck_in(directory.path())
        .args(["bundle", "main.lua"])
        .assert()
        .code(0)
        .stderr(predicate::str::contains("W007"));
    let bundle = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(bundle.contains("__luck_dynamic("), "{bundle}");
}

#[test]
fn bundle_with_dynamic_require_fails_in_error_mode() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "helper.lua", "return { value = 1 }\n");
    write_file(
        directory.path(),
        "main.lua",
        "\
local helper = require(\"helper\")
local name = \"helper\"
return helper.value + require(name).value
",
    );

    luck_in(directory.path())
        .args(["bundle", "main.lua", "--dynamic-require", "error"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("E002"));

    // The project setting reaches the one-shot command too.
    write_file(directory.path(), "luck.json", "{ \"dynamic_require\": \"error\" }\n");
    luck_in(directory.path())
        .args(["bundle", "main.lua"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("E002"));
}

/// `luck bundle main.lua` passes the entry's parent as the project root.
///
/// For a bare file name that root is empty, and no emitted path may then be
/// absolute, or the bundle would ship the build host's directory layout.
#[test]
fn bundle_of_a_bare_entry_name_emits_no_absolute_paths() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "helper.lua", "return { value = 1 }\n");
    write_file(
        directory.path(),
        "main.lua",
        "\
local helper = require(\"helper\")
return helper.value
",
    );

    let assert = luck_in(directory.path()).args(["bundle", "main.lua"]).assert().code(0);
    let bundle = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    let native_root = directory.path().to_string_lossy().to_string();
    let slash_root = native_root.replace('\\', "/");
    assert!(
        !bundle.contains(&native_root) && !bundle.contains(&slash_root),
        "absolute path leaked into the bundle:\n{bundle}"
    );
    assert!(bundle.contains("-- module \"helper\": helper.lua"), "{bundle}");
}
