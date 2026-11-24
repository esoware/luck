use std::path::{Path, PathBuf};
use std::time::Duration;

use luck_config::{Editorconfigs, Project};
use luck_formatter::FormatOptions;

use crate::common::{finishes_within, in_directory, project_with};

/// The indent width the `.editorconfig` files above `file_path` give it.
fn indent_width(file_path: &Path) -> u8 {
    Project::defaults(PathBuf::from("."))
        .format_options(Some(file_path), Some(&Editorconfigs::default()))
        .indent_width
}

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
    assert_eq!(
        indent_width(&project.path().join("main.lua")),
        2,
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
    assert_eq!(
        indent_width(&project.path().join("main.lua")),
        2,
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
    let width = in_directory(&project.path().join("src"), || indent_width(Path::new("a.lua")));
    assert_eq!(
        width, 3,
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
    assert_eq!(indent_width(&project.path().join("xb.lua")), 2, "`xb.lua` matches `xb`");
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
    let width = indent_width(&project.path().join("a.lua"));
    assert!(
        width == 2 || width == FormatOptions::default().indent_width,
        "the section either matches or is ignored as invalid: {width}"
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
    let is_finished = finishes_within(Duration::from_secs(10), move || indent_width(&file_path));
    assert!(is_finished, "a one-line section must not take exponential time to match");
}
