use std::fs;

use tempfile::TempDir;

use crate::common::{run, write_file};

#[test]
fn build_keeps_every_preamble_line_inside_its_comment() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "print(1)\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entry":"main.lua","output":"bundle.lua","preamble":"one\rerror(\"escaped\")\r\ntwo"}"#,
    );

    run(directory.path(), &["build", "--quiet"]);
    let bundle = fs::read_to_string(directory.path().join("bundle.lua")).unwrap();

    assert!(
        bundle.split(['\r', '\n']).all(|line| !line.starts_with("error(")),
        "a short comment ends at a CR, so a preamble line must not hold one:\n{bundle:?}"
    );
    assert!(
        bundle.starts_with(
            "\
-- one
-- error(\"escaped\")
-- two
"
        ),
        "{bundle:?}"
    );
}

#[test]
fn build_preamble_stays_after_a_shebang() {
    let directory = TempDir::new().unwrap();
    write_file(
        directory.path(),
        "main.lua",
        "\
#!/usr/bin/env luau
print(1)
",
    );
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entry":"main.lua","output":"bundle.lua","preamble":"hello"}"#,
    );

    run(directory.path(), &["build", "--quiet"]);
    let bundle = fs::read_to_string(directory.path().join("bundle.lua")).unwrap();

    assert!(
        bundle.starts_with("#!"),
        "Luau only skips a shebang on the first line of a chunk, so it must stay there:\n{bundle}"
    );
}
