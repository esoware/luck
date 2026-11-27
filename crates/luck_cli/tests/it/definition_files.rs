//! A `.d.luau` file is a definition file by its name, wherever a command
//! meets it, and no module.

use std::fs;

use tempfile::TempDir;

use crate::common::{run, stderr_text, stdout_text, write_file};

const APP: &str = "declare function greet(name: string): string\n";

#[test]
fn fmt_formats_a_definition_file_as_one() {
    let directory = TempDir::new().unwrap();
    let path = write_file(
        directory.path(),
        "app.d.luau",
        "declare   function greet(name:string):string\n",
    );

    let output = run(directory.path(), &["fmt", "app.d.luau"]);

    assert!(output.status.success(), "{}", stderr_text(&output));
    assert_eq!(fs::read_to_string(path).expect("the file"), APP);
}

#[test]
fn lint_and_check_read_a_definition_file_named_outright_as_one() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", r#"{ "definitions": ["app.d.luau"] }"#);
    write_file(directory.path(), "app.d.luau", APP);

    let linted = run(directory.path(), &["lint", "app.d.luau"]);
    assert!(linted.status.success(), "{}", stderr_text(&linted));

    let checked = run(directory.path(), &["check", "app.d.luau"]);
    assert!(checked.status.success(), "{}", stderr_text(&checked));
    assert!(stderr_text(&checked).contains("ok: 1 file(s) checked"), "{}", stdout_text(&checked));
}

#[test]
fn lint_reports_a_definition_file_s_syntax_errors() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "app.d.luau", "declare function greet(name: string: string\n");

    let output = run(directory.path(), &["lint", "app.d.luau"]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("[E008]"), "{stderr}");
}

#[test]
fn a_require_of_a_definition_file_is_an_error() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "app.d.luau", APP);
    let main = "\
local app = require(\"./app.d\")
print(app)
";
    write_file(directory.path(), "main.luau", main);

    let output = run(directory.path(), &["bundle", "main.luau"]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("module not found: `./app.d`"), "{stderr}");
    assert!(
        stderr.contains("is a definition file, which declares types and is no module"),
        "{stderr}"
    );
}

#[test]
fn lint_and_check_give_an_override_s_definitions_to_the_files_it_matches_alone() {
    let directory = TempDir::new().unwrap();
    write_file(
        directory.path(),
        "luck.json",
        r#"{ "overrides": [{ "files": ["**/*.spec.luau"], "definitions": ["types"] }] }"#,
    );
    write_file(
        directory.path(),
        "types/spec.d.luau",
        "declare function describe(name: string, body: () -> ()): ()\n",
    );
    let uses_describe = "\
--!strict
describe(\"adds\", function() end)
";
    write_file(directory.path(), "src/add.spec.luau", uses_describe);
    write_file(directory.path(), "src/add.luau", uses_describe);

    for command in ["lint", "check"] {
        let spec = run(directory.path(), &[command, "src/add.spec.luau"]);
        assert!(spec.status.success(), "{command}: {}", stderr_text(&spec));

        let source = run(directory.path(), &[command, "src/add.luau"]);
        let stderr = stderr_text(&source);
        assert_eq!(source.status.code(), Some(1), "{command}: {stderr}");
        assert!(stderr.contains("describe"), "{command}: {stderr}");
    }
}
