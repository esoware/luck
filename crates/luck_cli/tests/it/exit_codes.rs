//! These spawn the real `luck` binary, so they reach `main.rs`'s worker thread
//! and clap's own usage-error exit, which unit tests can't.

use core::time::Duration;
use std::fs;
use std::path::Path;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, write_file};

#[test]
fn lint_clean_file_exits_success() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "clean.lua", "print(\"hello world\")\n");

    luck_in(directory.path()).args(["lint", "clean.lua"]).assert().code(0i32);
}

#[test]
fn lint_undefined_variable_exits_failure() {
    let directory = TempDir::new().unwrap();
    // `undefined_variable` is on by default as an error.
    write_file(directory.path(), "bad.lua", "return missingglobal\n");

    luck_in(directory.path())
        .args(["lint", "bad.lua"])
        .assert()
        .code(1i32)
        .stderr(predicate::str::contains("undefined_variable"));
}

#[test]
fn check_parse_error_exits_failure() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "broken.lua", "local x = 1 +\n");

    luck_in(directory.path()).args(["check", "broken.lua"]).assert().code(1i32);
}

#[test]
fn check_valid_file_exits_success() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "ok.lua", "local x = 1\nreturn x\n");

    luck_in(directory.path()).args(["check", "ok.lua"]).assert().code(0i32);
}

#[test]
fn fmt_check_unformatted_exits_failure() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "messy.lua", "local    x=1\n");

    luck_in(directory.path()).args(["fmt", "--check", "messy.lua"]).assert().code(1i32);
}

#[test]
fn fmt_check_formatted_exits_success() {
    let directory = TempDir::new().unwrap();
    let path = write_file(directory.path(), "tidy.lua", "local    x=1\n");

    // Format with luck first, so the check doesn't depend on the formatter's
    // style choices.
    luck_in(directory.path()).args(["fmt", "tidy.lua"]).assert().code(0i32);
    assert!(fs::read_to_string(&path).unwrap().contains("local x = 1"));

    luck_in(directory.path()).args(["fmt", "--check", "tidy.lua"]).assert().code(0i32);
}

#[test]
fn unknown_subcommand_exits_usage() {
    let directory = TempDir::new().unwrap();

    luck_in(directory.path())
        .arg("frobnicate")
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("frobnicate"));
}

#[test]
fn invalid_flag_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "clean.lua", "print(\"hi\")\n");

    luck_in(directory.path())
        .args(["lint", "--nonexistent-flag", "clean.lua"])
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("--nonexistent-flag"));
}

#[test]
fn invalid_config_unknown_key_exits_usage() {
    let directory = TempDir::new().unwrap();
    // `deny_unknown_fields` turns any unrecognized top-level key into a hard
    // config error, which the CLI maps to `Exit::Usage`.
    write_file(directory.path(), "luck.json", "{ \"not_a_real_key\": true }\n");
    write_file(directory.path(), "clean.lua", "print(\"hi\")\n");

    luck_in(directory.path())
        .args(["check", "clean.lua"])
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("error: "));
}

/// Target resolution has to happen before the parallel section, because a
/// worker reporting the error would deadlock on the stdio locks the main
/// thread holds there.
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
        .code(2i32)
        .stderr(predicate::str::contains("invalid target `lua99`"));
}

#[test]
fn lint_bad_dialect_with_multiple_files_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_bad_dialect_project(directory.path());

    luck_in(directory.path())
        .arg("lint")
        .timeout(Duration::from_secs(60))
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("invalid target `lua99`"));
}

#[test]
fn check_bad_dialect_with_multiple_files_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_bad_dialect_project(directory.path());

    luck_in(directory.path())
        .arg("check")
        .timeout(Duration::from_secs(60))
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("invalid target `lua99`"));
}

/// `--fix` over stdin re-lints the fixed text, so spans index that text.
///
/// A fix that grows the file pushes spans past the end of the original, and a
/// renderer panic also exits 1, so the rendered location is the proof.
#[test]
fn lint_stdin_fix_renders_spans_past_the_end_of_the_original() {
    let directory = TempDir::new().unwrap();

    luck_in(directory.path())
        .args(["lint", "--fix", "--stdin-filepath", "buffer.lua"])
        .write_stdin("local unused = 1\nreturn missingglobal")
        .assert()
        .code(1i32)
        .stdout("local _unused = 1\nreturn missingglobal")
        .stderr(predicate::str::contains("buffer.lua:2:8"));
}

/// A fix that shrinks the file keeps every span in bounds of the original, so
/// rendering against the original misplaces the snippet without any panic.
#[test]
fn lint_stdin_fix_renders_spans_against_the_shortened_text() {
    let directory = TempDir::new().unwrap();

    luck_in(directory.path())
        .args(["lint", "--fix", "--stdin-filepath", "buffer.lua"])
        .args(["-W", "redundant_nil_init"])
        .write_stdin("local a = nil\nreturn a, missingglobal\n")
        .assert()
        .code(1i32)
        .stdout("local a\nreturn a, missingglobal\n")
        .stderr(predicate::str::contains("buffer.lua:2:11"));
}

/// An empty `entries` list is a config error, not a panic or a silent exit 0.
#[test]
fn build_empty_entries_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", "{ \"entries\": [] }\n");

    luck_in(directory.path())
        .arg("build")
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("`entries` in `luck.json` is empty"));

    luck_in(directory.path())
        .args(["build", "--quiet"])
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("`entries` in `luck.json` is empty"));
}

#[test]
fn fmt_range_formats_only_covered_stmt() {
    let directory = TempDir::new().unwrap();
    // Two unformatted statements; the range covers only the second line.
    let path = write_file(directory.path(), "partial.lua", "local    a=1\nlocal    b=2\n");

    luck_in(directory.path())
        .args(["fmt", "--range-start", "13", "partial.lua"])
        .assert()
        .code(0i32);
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("local    a=1"), "{contents}");
    assert!(contents.contains("local b = 2"), "{contents}");
}

#[test]
fn fmt_range_with_multiple_files_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "a.lua", "local a = 1\n");
    write_file(directory.path(), "b.lua", "local b = 2\n");

    luck_in(directory.path())
        .args(["fmt", "--range-start", "0", "a.lua", "b.lua"])
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("exactly one input file"));
}

#[test]
fn fmt_range_reversed_bounds_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "a.lua", "local a = 1\n");

    luck_in(directory.path())
        .args(["fmt", "--range-start", "9", "--range-end", "2", "a.lua"])
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("invalid format range"));
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
            .code(2i32)
            .stderr(predicate::str::contains("invalid value 'lua99' for '--target <TARGET>'"))
            .stderr(predicate::str::contains("invalid target `lua99`"))
            .stderr(predicate::str::contains("not_a_real_key").not());
    }
}

#[test]
fn bad_init_target_exits_usage_and_writes_nothing() {
    let directory = TempDir::new().unwrap();

    luck_in(directory.path())
        .args(["init", "-t", "lua99"])
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("invalid value 'lua99' for '--target <TARGET>'"));
    assert!(!directory.path().join("luck.json").exists());
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
            .code(2i32)
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
        "local helper = require(\"helper\")\nlocal name = \"helper\"\nreturn helper.value + require(name).value\n",
    );

    let assert = luck_in(directory.path())
        .args(["bundle", "main.lua"])
        .assert()
        .code(0i32)
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
        "local helper = require(\"helper\")\nlocal name = \"helper\"\nreturn helper.value + require(name).value\n",
    );

    luck_in(directory.path())
        .args(["bundle", "main.lua", "--dynamic-require", "error"])
        .assert()
        .code(1i32)
        .stderr(predicate::str::contains("E002"));

    // The project setting reaches the one-shot command too.
    write_file(directory.path(), "luck.json", "{ \"dynamic_require\": \"error\" }\n");
    luck_in(directory.path())
        .args(["bundle", "main.lua"])
        .assert()
        .code(1i32)
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
        "local helper = require(\"helper\")\nreturn helper.value\n",
    );

    let assert = luck_in(directory.path()).args(["bundle", "main.lua"]).assert().code(0i32);
    let bundle = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    let native_root = directory.path().to_string_lossy().to_string();
    let slash_root = native_root.replace('\\', "/");
    assert!(
        !bundle.contains(&native_root) && !bundle.contains(&slash_root),
        "absolute path leaked into the bundle:\n{bundle}"
    );
    assert!(bundle.contains("-- module \"helper\": helper.lua"), "{bundle}");
}
