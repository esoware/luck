use core::time::Duration;
use std::path::Path;

use luck_config::resolved_format_config;

use crate::common::{finishes_within, in_directory, project_with};

/// The spec reads a key with nothing after `=` as a valid pair with an empty
/// value.
#[test]
fn editorconfig_empty_value_keeps_the_other_properties() {
    let project = project_with(&[(
        ".editorconfig",
        "\
[*.lua]
indent_size = 2
charset =
",
    )]);
    let config = resolved_format_config(None, Some(&project.path().join("main.lua")));
    assert_eq!(
        config.indent_width,
        Some(2),
        "a valid `key =` pair must not discard the section's other properties"
    );
}

#[test]
fn editorconfig_bom_before_the_first_section_keeps_its_properties() {
    let project = project_with(&[(
        ".editorconfig",
        "\
\u{feff}[*.lua]
indent_size = 2
",
    )]);
    let config = resolved_format_config(None, Some(&project.path().join("main.lua")));
    assert_eq!(
        config.indent_width,
        Some(2),
        "a UTF-8 BOM before the first section header must be ignored"
    );
}

/// A section with a `/` is relative to the `.editorconfig` that holds it, and
/// the CLI passes the paths its walk yields, which are relative to cwd.
#[test]
fn editorconfig_slash_section_applies_to_a_path_relative_to_a_subdirectory() {
    let project = project_with(&[
        (
            ".editorconfig",
            "\
root = true
[src/*.lua]
indent_size = 3
",
        ),
        ("src/a.lua", ""),
    ]);
    let config = in_directory(&project.path().join("src"), || {
        resolved_format_config(None, Some(Path::new("a.lua")))
    });
    assert_eq!(
        config.indent_width,
        Some(3),
        "`src/a.lua` matches `[src/*.lua]` however the working directory spells it"
    );
}

#[test]
fn editorconfig_alternative_after_a_starred_one_still_matches() {
    let project = project_with(&[(
        ".editorconfig",
        "\
root = true
[{a*b,xb}.lua]
indent_size = 2
",
    )]);
    let config = resolved_format_config(None, Some(&project.path().join("xb.lua")));
    assert_eq!(config.indent_width, Some(2), "`xb.lua` matches the `xb` alternative");
}

#[test]
fn deeply_nested_editorconfig_section_does_not_crash() {
    let section = format!(
        "\
root = true
[{}a{}]
indent_size = 2
",
        "{a,".repeat(5000),
        "}".repeat(5000)
    );
    let project = project_with(&[(".editorconfig", &section)]);
    let config = resolved_format_config(None, Some(&project.path().join("a.lua")));
    assert!(
        matches!(config.indent_width, None | Some(2)),
        "the section either matches or is ignored as invalid: {config:?}"
    );
}

#[test]
fn editorconfig_alternation_section_matches_promptly() {
    let section = format!(
        "\
root = true
[b{}.lua]
indent_size = 2
",
        "{a,aa}".repeat(40)
    );
    let project = project_with(&[(".editorconfig", &section)]);
    let file_path = project.path().join(format!("{}.lua", "a".repeat(60)));
    let is_finished = finishes_within(Duration::from_secs(10), move || {
        resolved_format_config(None, Some(&file_path))
    });
    assert!(is_finished, "a one-line section must not take exponential time to match");
}
