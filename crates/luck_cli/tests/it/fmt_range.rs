use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, run, write_file};

#[test]
fn fmt_range_formats_only_covered_stmt() {
    let directory = TempDir::new().unwrap();
    // Two unformatted statements; the range covers only the second line.
    let path = write_file(
        directory.path(),
        "partial.lua",
        "\
local    a=1
local    b=2
",
    );

    luck_in(directory.path()).args(["fmt", "--range-start", "13", "partial.lua"]).assert().code(0);
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("local    a=1"), "{contents}");
    assert!(contents.contains("local b = 2"), "{contents}");
}

#[test]
fn fmt_range_leaves_crlf_outside_the_range_verbatim() {
    let directory = TempDir::new().unwrap();
    let path = write_file(directory.path(), "main.lua", "local    a=1\r\nlocal    b=2\r\n");

    run(directory.path(), &["fmt", "--range-start", "14", "main.lua"]);
    let formatted = fs::read_to_string(path).unwrap();

    assert!(
        formatted.starts_with("local    a=1\r\n"),
        "a statement outside the range is copied verbatim, line ending included:\n{formatted:?}"
    );
}

#[test]
fn fmt_range_with_multiple_files_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "a.lua", "local a = 1\n");
    write_file(directory.path(), "b.lua", "local b = 2\n");

    luck_in(directory.path())
        .args(["fmt", "--range-start", "0", "a.lua", "b.lua"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("exactly one input file"));
}

#[test]
fn fmt_range_reversed_bounds_exits_usage() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "a.lua", "local a = 1\n");

    luck_in(directory.path())
        .args(["fmt", "--range-start", "9", "--range-end", "2", "a.lua"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid format range"));
}
