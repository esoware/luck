//! What `lint` prints and how its level flags decide the exit code.

use std::path::Path;

use luck_core::normalize_path;
use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, write_file};

/// `unused_variable` is a warning by default and `undefined_variable` an
/// error.
const WARNING_AND_ERROR: &str = "local unused = 1\nprint(undefined_name)\n";
const WARNING_ONLY: &str = "local unused = 1\n";

#[test]
fn quiet_prints_errors_only() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "main.lua", WARNING_AND_ERROR);

    luck_in(project.path())
        .args(["lint", "main.lua", "--quiet"])
        .assert()
        .code(1i32)
        .stderr(predicate::str::contains("undefined_variable"))
        .stderr(predicate::str::contains("unused_variable").not());
}

#[test]
fn quiet_still_counts_warnings_toward_the_exit_code() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "main.lua", WARNING_ONLY);

    luck_in(project.path()).args(["lint", "main.lua", "--quiet"]).assert().code(0i32).stderr("");
    luck_in(project.path())
        .args(["lint", "main.lua", "--quiet", "--deny-warnings"])
        .assert()
        .code(1i32)
        .stderr("error: 1 warning(s) exceed the limit of 0\n");
    luck_in(project.path())
        .args(["lint", "main.lua", "--quiet", "--max-warnings", "1"])
        .assert()
        .code(0i32)
        .stderr("");
}

#[test]
fn deny_makes_a_category_s_rules_errors_and_warn_makes_them_warnings() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "main.lua", WARNING_AND_ERROR);
    write_file(project.path(), "warning.lua", WARNING_ONLY);

    luck_in(project.path()).args(["lint", "warning.lua"]).assert().code(0i32);
    luck_in(project.path())
        .args(["lint", "warning.lua", "-D", "correctness"])
        .assert()
        .code(1i32)
        .stderr(predicate::str::contains("1 error(s), 0 warning(s)"));
    luck_in(project.path())
        .args(["lint", "main.lua", "-W", "correctness"])
        .assert()
        .code(0i32)
        .stderr(predicate::str::contains("0 error(s), 2 warning(s)"));
}

#[test]
fn level_flags_apply_in_the_order_given() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "main.lua", WARNING_ONLY);

    luck_in(project.path())
        .args(["lint", "main.lua", "-D", "unused_variable", "-A", "unused_variable"])
        .assert()
        .code(0i32)
        .stderr("");
    luck_in(project.path())
        .args(["lint", "main.lua", "-A", "unused_variable", "-D", "unused_variable"])
        .assert()
        .code(1i32);
}

#[test]
fn rule_level_wins_over_a_category_flag() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "luck.json", r#"{"lint":{"rules":{"unused_variable":"off"}}}"#);
    write_file(project.path(), "main.lua", WARNING_ONLY);

    luck_in(project.path())
        .args(["lint", "main.lua", "-D", "correctness"])
        .assert()
        .code(0i32)
        .stderr("");
}

/// The path is absolute, so a workflow step with a `working-directory` still
/// annotates the right file.
#[test]
fn github_format_prints_annotations_on_stdout() {
    let project = TempDir::new().expect("project");
    let main_path = write_file(project.path(), "src/main.lua", WARNING_AND_ERROR);
    let file = normalize_path(&main_path).replace(':', "%3A");

    luck_in(&project.path().join("src"))
        .args(["lint", "--format", "github"])
        .assert()
        .code(1i32)
        .stdout(predicate::str::contains(format!(
            "::warning file={file},line=1,endLine=1,col=7,endColumn=13,\
             title=unused_variable::unused variable `unused`",
        )))
        .stdout(predicate::str::contains(format!(
            "::error file={file},line=2,endLine=2,col=7,endColumn=21,\
             title=undefined_variable::",
        )));
}

#[test]
fn json_format_carries_lines_and_columns() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "main.lua", WARNING_ONLY);

    luck_in(project.path())
        .args(["lint", "main.lua", "--format", "json"])
        .assert()
        .code(0i32)
        .stdout(predicate::str::contains(r#""line":1"#))
        .stdout(predicate::str::contains(r#""column":7"#))
        .stdout(predicate::str::contains(r#""end_line":1"#))
        .stdout(predicate::str::contains(r#""end_column":13"#));
}

/// The printed config is a `lint` section, so feeding it back changes nothing.
#[test]
fn print_config_output_pastes_back_into_luck_json() {
    let project = TempDir::new().expect("project");
    write_file(
        project.path(),
        "luck.json",
        r#"{"lint":{"rules":{"cyclomatic_complexity":["warn",{"max":8}]}}}"#,
    );

    let with_flags = printed_config(
        project.path(),
        &["-D", "style", "-A", "unused_variable", "--global", "vim"],
    );
    assert!(with_flags.contains(r#""style": "error""#), "{with_flags}");
    assert!(with_flags.contains(r#""unused_variable": "off""#), "{with_flags}");
    assert!(with_flags.contains(r#""max": 8"#), "{with_flags}");

    write_file(project.path(), "luck.json", &format!(r#"{{"lint":{with_flags}}}"#));
    assert_eq!(printed_config(project.path(), &[]), with_flags);
}

/// Each file is linted under its nearest `luck.json`, so the config printed
/// for a path is that one, not the working directory's.
#[test]
fn print_config_shows_the_config_of_the_path_given() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "luck.json", r#"{"lint":{"rules":{"empty_block":"warn"}}}"#);
    write_file(project.path(), "pkg/luck.json", r#"{"lint":{"rules":{"empty_block":"error"}}}"#);
    write_file(project.path(), "pkg/a.lua", WARNING_ONLY);

    assert!(printed_config(project.path(), &[]).contains(r#""empty_block": "warn""#));
    for args in [["pkg"].as_slice(), &["pkg/a.lua"], &["--stdin-filepath", "pkg/a.lua"]] {
        let config = printed_config(project.path(), args);
        assert!(config.contains(r#""empty_block": "error""#), "{args:?}: {config}");
    }
    luck_in(project.path())
        .args(["lint", "--print-config", "pkg", "pkg/a.lua"])
        .assert()
        .code(2i32)
        .stderr("error: `--print-config` takes at most one path\n");
}

fn printed_config(root: &Path, args: &[&str]) -> String {
    let output =
        luck_in(root).args(["lint", "--print-config"]).args(args).output().expect("lint runs");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("JSON is UTF-8")
}
