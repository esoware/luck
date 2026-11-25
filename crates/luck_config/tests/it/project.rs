use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_file;
#[cfg(windows)]
use std::os::windows::fs::symlink_file;
use std::path::{Path, PathBuf};

use luck_bundler::DynamicRequire;
use luck_config::{Editorconfigs, Project};
use luck_formatter::{FormatOptions, IndentStyle};
use luck_linter::{Category, LintConfig, RuleId, RuleLevel};
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_stdlib::Variant;

use crate::common::{load_error, loaded, project_with};

#[test]
fn defaults_take_every_lua_and_luau_file_as_luau() {
    let project = Project::defaults(PathBuf::from("."));
    assert_eq!(project.variant(), Variant::Standalone);
    assert_eq!(project.stdlib().variant(), Variant::Standalone);
    assert!(project.filter().is_included(Path::new("src/a.lua")));
    assert!(project.filter().is_included(Path::new("src/a.luau")));
    assert_eq!(*project.linter_for(None).config(), LintConfig::default());
    assert_eq!(project.files(), Vec::<PathBuf>::new());
}

#[test]
fn the_variant_key_picks_the_library_every_file_gets() {
    let (_roblox_directory, roblox) = loaded(&[("luck.json", r#"{ "variant": "roblox" }"#)]);
    assert_eq!(roblox.variant(), Variant::Roblox);
    assert!(roblox.stdlib().library().lookup(["game"]).is_some());

    let (_luau_directory, luau) = loaded(&[("luck.json", r#"{ "variant": "standalone" }"#)]);
    assert_eq!(luau.variant(), Variant::Standalone);
    assert!(luau.stdlib().library().lookup(["game"]).is_none());
}

#[test]
fn rejects_a_variant_on_its_value() {
    let text = r#"{ "variant": "bogus" }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert_eq!(diag.code, "E014");
    assert!(
        diag.message.contains("unknown variant `bogus`, expected `standalone` or `roblox`"),
        "{diag:?}"
    );
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""bogus""#[..]));
}

#[test]
fn rejects_entry_beside_entries() {
    let directory = project_with(&[(
        "luck.json",
        r#"{ "entry": "a.lua", "entries": [{ "entry": "b.lua", "output": "c.lua" }] }"#,
    )]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert_eq!(diag.code, "E014");
    assert!(diag.message.contains("`entry` and `entries` can't both be set"), "{diag:?}");
}

#[test]
fn rejects_empty_entries_on_the_list() {
    let text = r#"{ "entries": [] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert!(diag.message.contains("`entries` is empty"), "{diag:?}");
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"["[..]));
}

#[test]
fn an_override_layers_its_lint_settings_over_the_files_it_matches() {
    let (directory, project) = loaded(&[(
        "luck.json",
        r#"{
            "lint": { "rules": { "shadowing": "warn" }, "extra_globals": ["a"] },
            "overrides": [
                {
                    "files": ["spec/**"],
                    "exclude_files": ["spec/fixtures/**"],
                    "lint": { "rules": { "shadowing": "off" }, "extra_globals": ["it"] }
                },
                { "files": ["spec/slow/**"], "lint": { "categories": { "style": "error" } } }
            ]
        }"#,
    )]);
    let linter_at = |relative: &str| project.linter_for(Some(&directory.path().join(relative)));

    let base = linter_at("src/a.lua");
    assert_eq!(base.config().rules.level(RuleId::Shadowing), Some(RuleLevel::Warn));

    let spec = linter_at("spec/a.lua");
    assert_eq!(spec.config().rules.level(RuleId::Shadowing), Some(RuleLevel::Off));
    assert_eq!(spec.config().extra_globals, ["a", "it"]);
    assert_eq!(spec.config().categories.level(Category::Style), None);

    let slow = linter_at("spec/slow/a.lua");
    assert_eq!(slow.config().rules.level(RuleId::Shadowing), Some(RuleLevel::Off));
    assert_eq!(slow.config().categories.level(Category::Style), Some(RuleLevel::Error));

    let fixture = linter_at("spec/fixtures/a.lua");
    assert_eq!(fixture.config().rules.level(RuleId::Shadowing), Some(RuleLevel::Warn));
}

#[test]
fn an_extended_override_matches_from_the_loaded_config_s_directory() {
    let (directory, project) = loaded(&[
        (
            "shared/base.json",
            r#"{ "overrides": [{ "files": ["spec/**"], "lint": { "extra_globals": ["it"] } }] }"#,
        ),
        ("luck.json", r#"{ "extends": ["shared/base.json"] }"#),
    ]);
    let linter = project.linter_for(Some(&directory.path().join("spec/a.lua")));
    assert_eq!(linter.config().extra_globals, ["it"]);
}

/// A symlink takes the project of its own directory, so its `include` and
/// `overrides` globs match it there too, wherever it leads.
#[test]
fn a_symlink_matches_the_globs_where_it_is_and_not_where_it_leads() {
    let (directory, project) = loaded(&[(
        "luck.json",
        r#"{
            "include": ["src/**"],
            "overrides": [{ "files": ["src/**"], "lint": { "extra_globals": ["it"] } }]
        }"#,
    )]);
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let target = elsewhere.path().join("b.lua");
    fs::write(&target, "").expect("write b.lua");
    fs::create_dir_all(directory.path().join("src")).expect("mkdir src");
    let link = directory.path().join("src/b.lua");
    // Windows lets only an administrator or developer mode make one.
    if symlink_file(&target, &link).is_err() {
        return;
    }
    assert!(project.filter().is_included(&link));
    assert!(!project.filter().is_included(&target));
    assert_eq!(project.linter_for(Some(&link)).config().extra_globals, ["it"]);
}

#[test]
fn an_overlay_wins_over_every_override() {
    let (directory, mut project) = loaded(&[(
        "luck.json",
        r#"{ "overrides": [{ "files": ["spec/**"], "lint": { "rules": { "shadowing": "warn" } } }] }"#,
    )]);
    let mut overlay = LintConfig::default();
    overlay.rules.set_level(RuleId::Shadowing, RuleLevel::Error);
    overlay.extra_globals.push("app".to_owned());
    project.overlay_lint(&overlay);
    for relative in ["src/a.lua", "spec/a.lua"] {
        let linter = project.linter_for(Some(&directory.path().join(relative)));
        let lint = linter.config();
        assert_eq!(lint.rules.level(RuleId::Shadowing), Some(RuleLevel::Error), "{relative}");
        assert_eq!(lint.extra_globals, ["app"], "{relative}");
    }
}

#[test]
fn format_layers_defaults_then_editorconfig_then_the_section_then_overrides() {
    let (directory, project) = loaded(&[
        (
            "luck.json",
            r#"{
                "format": { "indent_width": 3 },
                "overrides": [{ "files": ["spec/**"], "format": { "line_width": 80 } }]
            }"#,
        ),
        (
            ".editorconfig",
            "\
[*.lua]
indent_style = space
indent_size = 2
max_line_length = 90
",
        ),
    ]);
    let editorconfigs = Editorconfigs::default();
    let options_at = |relative: &str| {
        project.format_options(Some(&directory.path().join(relative)), Some(&editorconfigs))
    };

    let source = options_at("src/a.lua");
    assert_eq!(source.indent_style, IndentStyle::Spaces);
    assert_eq!(source.indent_width, 3);
    assert_eq!(source.line_width, 90);
    assert_eq!(options_at("spec/a.lua").line_width, 80);

    let without_editorconfig = project.format_options(Some(&directory.path().join("a.lua")), None);
    assert_eq!(without_editorconfig.indent_style, FormatOptions::default().indent_style);
    assert_eq!(without_editorconfig.indent_width, 3);
    assert_eq!(project.format_options(None, Some(&editorconfigs)).indent_width, 3);
}

#[test]
fn records_every_config_file_it_read() {
    let (directory, project) =
        loaded(&[("base.json", "{}"), ("luck.json", r#"{ "extends": ["base.json"] }"#)]);
    let canonical =
        |name: &str| FileSystemOs.canonicalize(&directory.path().join(name)).expect("exists");
    assert_eq!(project.files(), [canonical("luck.json"), canonical("base.json")]);
}

const APP: &str = r#"
[globals.app]
kind = "namespace"

[globals.app.members.draw]
kind = "function"
is_read_only = false
"#;

#[test]
fn std_definitions_layer_over_the_variant_s_library() {
    let (directory, project) = loaded(&[
        ("std/app.toml", APP),
        ("luck.json", r#"{ "variant": "roblox", "std": ["std/app.toml"] }"#),
    ]);
    let stdlib = project.stdlib();
    assert_eq!(stdlib.variant(), Variant::Roblox);
    assert!(stdlib.library().lookup(["app", "draw"]).is_some());
    assert!(stdlib.library().lookup(["game"]).is_some());
    assert!(stdlib.library().lookup(["print"]).is_some());
    let std_file =
        FileSystemOs.canonicalize(&directory.path().join("std/app.toml")).expect("exists");
    assert!(project.files().contains(&std_file), "{:?}", project.files());
}

#[test]
fn a_std_path_is_relative_to_the_file_that_names_it() {
    let (_directory, project) = loaded(&[
        ("shared/app.toml", APP),
        ("shared/base.json", r#"{ "std": ["app.toml"] }"#),
        ("luck.json", r#"{ "extends": ["shared/base.json"], "std": ["shared/app.toml"] }"#),
    ]);
    assert!(project.stdlib().library().lookup(["app"]).is_some());
    assert_eq!(project.files().len(), 3, "one std file, read once: {:?}", project.files());
}

#[test]
fn a_std_file_that_cannot_be_read_is_reported_on_its_name() {
    let text = r#"{ "std": ["gone.toml"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (path, diag) = load_error(&directory.path().join("luck.json"));
    assert!(path.ends_with("luck.json"), "{path:?}");
    assert!(diag.message.contains("`std` names `gone.toml`, which can't be read"), "{diag:?}");
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""gone.toml""#[..]));
}

#[test]
fn an_invalid_std_definition_is_reported_in_it() {
    let definition = r#"
[globals.app]
kind = "namspace"
"#;
    let directory =
        project_with(&[("app.toml", definition), ("luck.json", r#"{ "std": ["app.toml"] }"#)]);
    let (path, diag) = load_error(&directory.path().join("luck.json"));
    assert!(path.ends_with("app.toml"), "{path:?}");
    assert_eq!(diag.code, "E014");
    assert!(diag.message.contains("unknown variant `namspace`"), "{diag:?}");
    assert!(diag.span.is_some(), "{diag:?}");
}

#[test]
fn a_std_definition_that_names_what_no_library_declares_is_reported_in_it() {
    let definition = r#"
[globals.screen]
kind = "property"
shape = "Sceen"
"#;
    let directory =
        project_with(&[("app.toml", definition), ("luck.json", r#"{ "std": ["app.toml"] }"#)]);
    let (path, diag) = load_error(&directory.path().join("luck.json"));
    assert!(path.ends_with("app.toml"), "{path:?}");
    assert_eq!(
        diag.message,
        "invalid config: `screen` names the shape `Sceen`, which no library declares"
    );
}

#[test]
fn builds_resolve_entries_profiles_and_outputs() {
    let (directory, project) = loaded(&[
        ("src/main.lua", "return 1"),
        (
            "luck.json",
            r#"{
                "entry": "src/main.lua", "output_dir": "dist", "minify": false,
                "profiles": { "release": { "minify": true } }
            }"#,
        ),
    ]);
    let builds = project.builds(Some("release")).expect("builds");
    let [build] = builds.as_slice() else { panic!("one build: {builds:?}") };
    assert!(build.minify);
    assert_eq!(build.output, directory.path().join("dist").join("main.lua"));
    let error = project.builds(Some("nightly")).unwrap_err();
    assert_eq!(error, "profile `nightly` isn't defined in `luck.json`");
}

#[test]
fn rejects_an_output_that_overwrites_the_entry() {
    let (_directory, project) = loaded(&[
        ("main.lua", "return 1"),
        ("luck.json", r#"{ "entry": "main.lua", "output_dir": "." }"#),
    ]);
    let builds = project.builds(None);
    assert!(builds.is_err(), "a bundle written over its own entry destroys it: {builds:?}");
}

#[test]
fn rejects_two_entries_writing_the_same_output() {
    let (_directory, project) = loaded(&[
        ("a.lua", "return 1"),
        ("b.lua", "return 2"),
        (
            "luck.json",
            r#"{ "entries": [
                { "entry": "a.lua", "output": "bundle.lua" },
                { "entry": "b.lua", "output": "bundle.lua" }
            ] }"#,
        ),
    ]);
    let builds = project.builds(None);
    assert!(builds.is_err(), "the second bundle would silently replace the first: {builds:?}");
}

#[test]
fn bundle_options_carry_the_root_and_dynamic_require() {
    let (directory, project) = loaded(&[("luck.json", r#"{ "dynamic_require": "error" }"#)]);
    let options = project.bundle_options();
    assert_eq!(options.dynamic_require, DynamicRequire::Error);
    assert_eq!(
        FileSystemOs.canonicalize_existing(options.project_root),
        FileSystemOs.canonicalize_existing(directory.path())
    );
}
