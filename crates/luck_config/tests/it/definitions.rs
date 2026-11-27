#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_dir;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;
use std::path::{Path, PathBuf};

use luck_config::{DefinitionSource, Definitions, Project};
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_stdlib::SecurityLevel;

use crate::common::{load_error, loaded, project_with};

/// Each definition file `definitions` loads, by its path relative to
/// `directory`, with `@roblox` as written.
fn loaded_names(directory: &Path, definitions: &Definitions) -> Vec<String> {
    let root = FileSystemOs.canonicalize(directory).expect("the project exists");
    definitions
        .sources()
        .iter()
        .map(|source| match source {
            DefinitionSource::Roblox => "@roblox".to_owned(),
            DefinitionSource::File { path, .. } => path
                .strip_prefix(&root)
                .expect("under the project")
                .to_string_lossy()
                .replace('\\', "/"),
        })
        .collect()
}

/// The files `definitions` loads in a project of `files`, `config` its
/// `luck.json`.
fn loaded_with(files: &[(&str, &str)], config: &str) -> Vec<String> {
    let mut all = files.to_vec();
    all.push(("luck.json", config));
    let (directory, project) = loaded(&all);
    loaded_names(directory.path(), project.definitions())
}

const TREE: [(&str, &str); 6] = [
    ("types/b.d.luau", ""),
    ("types/a.d.luau", ""),
    ("types/ui/z.d.luau", ""),
    ("types/ui/deep/y.d.luau", ""),
    ("types/core/c.d.luau", ""),
    ("types/readme.luau", ""),
];

#[test]
fn a_glob_takes_each_directory_s_files_sorted_before_its_subdirectories() {
    let names = loaded_with(&TREE, r#"{ "definitions": ["types/**/*.d.luau"] }"#);
    assert_eq!(
        names,
        [
            "types/a.d.luau",
            "types/b.d.luau",
            "types/core/c.d.luau",
            "types/ui/z.d.luau",
            "types/ui/deep/y.d.luau",
        ]
    );
}

#[test]
fn a_directory_takes_every_definition_file_below_it_however_it_is_written() {
    let every = loaded_with(&TREE, r#"{ "definitions": ["types/**/*.d.luau"] }"#);
    for written in ["types", "./types/", "types/**"] {
        let config = format!(r#"{{ "definitions": ["{written}"] }}"#);
        assert_eq!(loaded_with(&TREE, &config), every, "{written}");
    }
}

#[test]
fn a_wildcard_takes_only_the_levels_it_spells() {
    let names = loaded_with(&TREE, r#"{ "definitions": ["types/*/*.d.luau"] }"#);
    assert_eq!(names, ["types/core/c.d.luau", "types/ui/z.d.luau"]);
}

#[test]
fn a_file_named_before_a_glob_loads_first_and_one_named_after_it_moves_last() {
    let pinned =
        loaded_with(&TREE, r#"{ "definitions": ["types/ui/z.d.luau", "types/*.d.luau"] }"#);
    assert_eq!(pinned, ["types/ui/z.d.luau", "types/a.d.luau", "types/b.d.luau"]);

    let moved = loaded_with(&TREE, r#"{ "definitions": ["types/**", "types/a.d.luau"] }"#);
    assert_eq!(moved.last().map(String::as_str), Some("types/a.d.luau"));
    assert_eq!(moved.len(), 5, "{moved:?}");
}

#[test]
fn the_first_glob_that_takes_a_file_places_it() {
    let names =
        loaded_with(&TREE, r#"{ "definitions": ["types/core/**", "types/**", "@roblox"] }"#);
    assert_eq!(
        names,
        [
            "types/core/c.d.luau",
            "types/a.d.luau",
            "types/b.d.luau",
            "types/ui/z.d.luau",
            "types/ui/deep/y.d.luau",
            "@roblox",
        ]
    );
}

#[test]
fn a_dropping_entry_removes_what_it_takes_and_a_later_entry_takes_it_again() {
    let dropped = loaded_with(&TREE, r#"{ "definitions": ["types/**", "!types/ui"] }"#);
    assert_eq!(dropped, ["types/a.d.luau", "types/b.d.luau", "types/core/c.d.luau"]);

    let again = loaded_with(
        &TREE,
        r#"{ "definitions": ["types/**", "!types/ui/**", "types/ui/deep/y.d.luau"] }"#,
    );
    assert_eq!(
        again,
        ["types/a.d.luau", "types/b.d.luau", "types/core/c.d.luau", "types/ui/deep/y.d.luau"]
    );

    let gone = loaded_with(&TREE, r#"{ "definitions": ["types/**", "!types/gone.d.luau"] }"#);
    assert_eq!(gone.len(), 5, "dropping a file that isn't there drops nothing: {gone:?}");
}

#[test]
fn a_wildcard_skips_a_name_starting_with_a_dot_the_glob_does_not_spell() {
    let files = [("types/a.d.luau", ""), ("types/.hidden/b.d.luau", ""), ("types/.c.d.luau", "")];
    assert_eq!(
        loaded_with(&files, r#"{ "definitions": ["types/**/*.d.luau"] }"#),
        ["types/a.d.luau"]
    );
    assert_eq!(loaded_with(&files, r#"{ "definitions": ["types"] }"#), ["types/a.d.luau"]);
    assert_eq!(
        loaded_with(&files, r#"{ "definitions": ["types/.hidden/*.d.luau"] }"#),
        ["types/.hidden/b.d.luau"]
    );
    assert_eq!(
        loaded_with(&files, r#"{ "definitions": ["types/.*.d.luau"] }"#),
        ["types/.c.d.luau"]
    );
}

#[test]
fn a_glob_that_takes_nothing_loads_nothing() {
    for glob in ["gen/*.d.luau", "types/*.d.lua", "types/**/missing/*.d.luau"] {
        let config = format!(r#"{{ "definitions": ["@roblox", "{glob}"] }}"#);
        assert_eq!(loaded_with(&TREE, &config), ["@roblox"], "{glob}");
    }
}

#[test]
fn a_part_with_a_glob_character_that_exists_as_written_is_literal() {
    let files = [("types/{GUID}/a.d.luau", ""), ("types/[old].d.luau", "")];
    assert_eq!(
        loaded_with(
            &files,
            r#"{ "definitions": ["types/{GUID}/*.d.luau", "types/[old].d.luau"] }"#
        ),
        ["types/{GUID}/a.d.luau", "types/[old].d.luau"]
    );
}

#[test]
fn a_missing_directory_is_reported_on_its_entry() {
    let text = r#"{ "definitions": ["types"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert!(diag.message.contains("`definitions` names `types`, which can't be read"), "{diag:?}");
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""types""#[..]));
}

#[test]
fn a_parent_directory_after_a_wildcard_is_reported_on_its_entry() {
    let text = r#"{ "definitions": ["types/**/../*.d.luau"] }"#;
    let directory = project_with(&[("luck.json", text), ("types/a.d.luau", "")]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert_eq!(
        diag.message,
        "invalid config: `definitions` names `types/**/../*.d.luau`, but a `..` can't follow a \
         wildcard"
    );
    assert_eq!(
        diag.span.map(|span| span.text(text.as_bytes())),
        Some(&br#""types/**/../*.d.luau""#[..])
    );
}

#[test]
fn an_invalid_glob_is_reported_on_its_entry() {
    let text = r#"{ "definitions": ["types/{a,b.d.luau"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert!(diag.message.contains("invalid glob pattern `{a,b.d.luau`"), "{diag:?}");
    assert_eq!(
        diag.span.map(|span| span.text(text.as_bytes())),
        Some(&br#""types/{a,b.d.luau""#[..])
    );
}

#[test]
fn dropping_a_definition_set_built_into_luck_is_reported_on_its_entry() {
    let text = r#"{ "definitions": ["@roblox", "!@roblox"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert!(
        diag.message.contains("`definitions` names `!@roblox`, but `!` drops only"),
        "{diag:?}"
    );
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""!@roblox""#[..]));
}

#[test]
fn an_extended_glob_takes_files_beside_the_file_that_names_it_and_the_child_drops_some() {
    let (directory, project) = loaded(&[
        ("shared/types/a.d.luau", ""),
        ("shared/types/b.d.luau", ""),
        ("shared/base.json", r#"{ "definitions": ["types/*.d.luau"] }"#),
        ("types/local.d.luau", ""),
        (
            "luck.json",
            r#"{ "extends": ["shared/base.json"], "definitions": ["!shared/types/a.d.luau", "types"] }"#,
        ),
    ]);
    assert_eq!(
        loaded_names(directory.path(), project.definitions()),
        ["shared/types/b.d.luau", "types/local.d.luau"]
    );
}

#[test]
fn every_definition_file_a_glob_takes_is_one_the_project_read() {
    let (directory, project) =
        loaded(&[("types/a.d.luau", APP), ("luck.json", r#"{ "definitions": ["types"] }"#)]);
    let file = FileSystemOs.canonicalize(&directory.path().join("types/a.d.luau")).expect("exists");
    assert!(project.files().contains(&file), "{:?}", project.files());
    assert!(project.definitions().stdlib().library().lookup(["app", "onMessage"]).is_some());
}

#[test]
fn a_glob_takes_a_definition_file_created_where_it_matches() {
    let (directory, project) =
        loaded(&[("luck.json", r#"{ "definitions": ["types/**/*.d.luau", "!types/old/**"] }"#)]);
    let at = |relative: &str| {
        let path = directory.path().join(relative);
        project.takes_definition_at(&luck_config::located_path(&FileSystemOs, &path))
    };
    assert!(project.reads_definition_files(), "a glob that takes nothing yet still reads");
    assert!(at("types/new.d.luau"));
    assert!(at("types/ui/new.d.luau"));
    assert!(!at("types/new.luau"));
    assert!(!at("types/.hidden/new.d.luau"));
    assert!(!at("other/new.d.luau"));

    let (_bare, bare) = loaded(&[("luck.json", r#"{ "definitions": ["@roblox"] }"#)]);
    assert!(!bare.reads_definition_files());
}

/// Windows lets only an administrator or developer mode make a symlink.
#[test]
fn a_glob_follows_a_symlinked_directory_and_enters_each_directory_once() {
    let elsewhere = tempfile::tempdir().expect("tempdir");
    std::fs::write(elsewhere.path().join("x.d.luau"), "").expect("write x.d.luau");
    let directory =
        project_with(&[("types/a.d.luau", ""), ("luck.json", r#"{ "definitions": ["types"] }"#)]);
    if symlink_dir(elsewhere.path(), directory.path().join("types/linked")).is_err()
        || symlink_dir(directory.path().join("types"), directory.path().join("types/loop")).is_err()
    {
        return;
    }
    let project =
        Project::load(&directory.path().join("luck.json"), FileSystemOs).expect("config loads");
    let target = FileSystemOs.canonicalize(&elsewhere.path().join("x.d.luau")).expect("exists");
    let paths: Vec<&PathBuf> = project
        .definitions()
        .sources()
        .iter()
        .filter_map(|source| match source {
            DefinitionSource::File { path, .. } => Some(path),
            DefinitionSource::Roblox => None,
        })
        .collect();
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert_eq!(paths[1], &target);
}

const APP: &str = "declare app: { onMessage: (text: string) -> () }";
const SPEC: &str = "declare function describe(name: string, body: () -> ()): ()";

#[test]
fn an_override_s_definitions_load_after_the_project_s_for_the_files_it_matches() {
    let (directory, project) = loaded(&[
        ("types/app.d.luau", APP),
        ("types/spec.d.luau", SPEC),
        (
            "luck.json",
            r#"{
                "definitions": ["@roblox", "types/app.d.luau"],
                "overrides": [
                    { "files": ["**/*.spec.luau"], "definitions": ["types/spec.d.luau"] },
                    { "files": ["scripts/**"], "definitions": ["!types/app.d.luau"] }
                ]
            }"#,
        ),
    ]);
    let at = |relative: &str| {
        let definitions = project.definitions_for(Some(&directory.path().join(relative)));
        loaded_names(directory.path(), &definitions)
    };
    assert_eq!(at("src/a.luau"), ["@roblox", "types/app.d.luau"]);
    assert_eq!(at("src/a.spec.luau"), ["@roblox", "types/app.d.luau", "types/spec.d.luau"]);
    assert_eq!(at("scripts/run.luau"), ["@roblox"]);
    assert_eq!(at("scripts/run.spec.luau"), ["@roblox", "types/spec.d.luau"]);

    let spec = project.definitions_for(Some(&directory.path().join("src/a.spec.luau")));
    assert!(spec.stdlib().library().lookup(["describe"]).is_some());
    let source = project.definitions_for(Some(&directory.path().join("src/a.luau")));
    assert!(source.stdlib().library().lookup(["describe"]).is_none());
}

#[test]
fn an_override_s_security_is_the_level_of_the_files_it_matches() {
    let (directory, project) = loaded(&[(
        "luck.json",
        r#"{
            "definitions": ["@roblox"],
            "overrides": [{ "files": ["plugin/**"], "security": "PluginSecurity" }]
        }"#,
    )]);
    let at =
        |relative: &str| project.definitions_for(Some(&directory.path().join(relative))).security();
    assert_eq!(at("game/a.luau"), SecurityLevel::None);
    assert_eq!(at("plugin/a.luau"), SecurityLevel::PluginSecurity);
}

#[test]
fn security_applies_to_a_project_an_override_of_which_loads_roblox() {
    let (directory, project) = loaded(&[(
        "luck.json",
        r#"{
            "security": "PluginSecurity",
            "overrides": [{ "files": ["plugin/**"], "definitions": ["@roblox"] }]
        }"#,
    )]);
    let at = |relative: &str| project.definitions_for(Some(&directory.path().join(relative)));
    assert_eq!(at("plugin/a.luau").security(), SecurityLevel::PluginSecurity);
    let script = at("tools/a.luau");
    assert!(!script.stdlib().library().has_roblox);
    assert_eq!(script.security(), SecurityLevel::None, "a file without Roblox's API has no level");

    let text = r#"{ "overrides": [{ "files": ["plugin/**"], "security": "PluginSecurity" }] }"#;
    let without = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&without.path().join("luck.json"));
    assert!(diag.message.contains("`security` applies only to a project that loads `@roblox`"));
}

#[test]
fn an_override_s_definitions_are_relative_to_the_file_that_names_them() {
    let (directory, project) = loaded(&[
        ("shared/spec.d.luau", SPEC),
        (
            "shared/base.json",
            r#"{ "overrides": [{ "files": ["spec/**"], "definitions": ["spec.d.luau"] }] }"#,
        ),
        ("luck.json", r#"{ "extends": ["shared/base.json"] }"#),
    ]);
    let definitions = project.definitions_for(Some(&directory.path().join("spec/a.luau")));
    assert_eq!(loaded_names(directory.path(), &definitions), ["shared/spec.d.luau"]);
    assert_eq!(project.definitions().sources(), []);
}

#[test]
fn an_override_s_definitions_error_is_placed_on_its_entry() {
    let text = r#"{
  "definitions": ["spec.d.luau"],
  "overrides": [{ "files": ["spec/**"], "definitions": ["spec.d.luau", "gone.d.luau"] }]
}"#;
    let directory = project_with(&[("luck.json", text), ("spec.d.luau", SPEC)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert!(diag.message.contains("`definitions` names `gone.d.luau`, which can't be read"));
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""gone.d.luau""#[..]));
}
