use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, write_file};

const MISTYPED_TYPE_CHECK: &str = "\
local x = 1
local ok = type(x) == \"strng\"
return ok
";

#[test]
fn lint_prints_a_rules_help() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", MISTYPED_TYPE_CHECK);

    luck_in(directory.path())
        .args(["lint", "main.lua"])
        .assert()
        .stderr(predicate::str::contains("did you mean `string`?"));
}

#[test]
fn lint_json_carries_a_rules_help() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", MISTYPED_TYPE_CHECK);

    let assert = luck_in(directory.path()).args(["lint", "main.lua", "--format", "json"]).assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let finding: serde_json::Value =
        serde_json::from_str(stdout.lines().next().expect("one finding")).expect("a JSON object");
    assert_eq!(finding["code"], "unknown_type", "{stdout}");
    assert!(
        finding["url"].as_str().is_some_and(|url| url.ends_with("/rules/unknown_type.rs")),
        "{stdout}"
    );
    assert_eq!(finding["help"], "did you mean `string`?", "{stdout}");
}

#[test]
fn lint_json_help_is_null_for_a_rule_with_none() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "local =\n");

    let assert = luck_in(directory.path()).args(["lint", "main.lua", "--format", "json"]).assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let finding: serde_json::Value =
        serde_json::from_str(stdout.lines().next().expect("one finding")).expect("a JSON object");
    assert_eq!(finding["code"], "E008", "{stdout}");
    assert!(finding["help"].is_null(), "{stdout}");
}

/// Lua reads a source as bytes, so a byte that isn't UTF-8 outside a string
/// is a parse error, shown in the file as a one-byte encoding shows it.
#[test]
fn a_dependency_that_is_not_utf8_reports_its_parse_error_in_place() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "return require(\"dep\")\n");
    fs::write(directory.path().join("dep.lua"), b"return \xFF\n").unwrap();

    luck_in(directory.path())
        .args(["bundle", "main.lua", "-t", "54"])
        .assert()
        .code(1i32)
        .stderr(predicate::str::contains("unexpected byte 0xFF"))
        .stderr(predicate::str::contains("dep.lua:1:8"))
        .stderr(predicate::str::contains("CacheMiss").not())
        .stderr(predicate::str::contains("Unable to fetch").not());
}

#[test]
fn an_entry_that_is_not_utf8_bundles_its_bytes_as_written() {
    let directory = TempDir::new().unwrap();
    fs::write(directory.path().join("main.lua"), b"-- caf\xE9\nreturn '\xFF\xFE'\n").unwrap();

    luck_in(directory.path())
        .args(["bundle", "main.lua", "-t", "51"])
        .assert()
        .success()
        .stdout(b"-- caf\xE9\nreturn '\xFF\xFE'\n".as_slice());
}

#[test]
fn minify_stats_are_ascii() {
    let directory = TempDir::new().unwrap();
    write_file(
        directory.path(),
        "main.lua",
        "\
local value = 1
return value
",
    );

    let assert =
        luck_in(directory.path()).args(["minify", "main.lua", "--stats"]).assert().code(0i32);
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.is_ascii() && stderr.contains(" -> "), "{stderr}");
}

#[test]
fn build_progress_is_ascii() {
    let directory = TempDir::new().unwrap();
    write_file(
        directory.path(),
        "main.lua",
        "\
local value = 1
return value
",
    );
    write_file(
        directory.path(),
        "luck.json",
        r#"{ "entry": "main.lua", "output": "out.lua", "minify": true }"#,
    );

    let assert = luck_in(directory.path()).arg("build").assert().code(0i32);
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.is_ascii(), "{stderr}");
    assert!(stderr.contains("->") && stderr.contains("done in"), "{stderr}");
}

#[test]
fn failed_build_reports_a_lowercase_error() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "local =\n");
    write_file(directory.path(), "luck.json", r#"{ "entry": "main.lua", "output": "out.lua" }"#);

    let assert = luck_in(directory.path()).arg("build").assert().code(1i32);
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("error:\u{1b}[0m build failed"), "{stderr}");
    assert!(!stderr.contains('\u{2717}'), "{stderr}");
}
