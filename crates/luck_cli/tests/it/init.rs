use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, run, stderr_text, write_file};

#[test]
fn init_quiet_prints_no_summary() {
    let directory = TempDir::new().unwrap();

    let output = run(directory.path(), &["init", "--quiet"]);

    assert_eq!(stderr_text(&output), "", "`--quiet` hides summaries");
}

#[test]
fn init_failure_leaves_no_partial_project() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "src", "a file where the source directory belongs\n");

    let output = run(directory.path(), &["init"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr_text(&output));

    assert!(
        !directory.path().join("luck.json").exists(),
        "a failed `init` must not leave a `luck.json` that blocks the next attempt"
    );
}

#[test]
fn bad_init_target_exits_usage_and_writes_nothing() {
    let directory = TempDir::new().unwrap();

    luck_in(directory.path())
        .args(["init", "-t", "lua99"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid value 'lua99' for '--target <TARGET>'"));
    assert!(!directory.path().join("luck.json").exists());
}
