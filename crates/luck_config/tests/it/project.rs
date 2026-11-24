use std::fs;
use std::path::{Path, PathBuf};

use luck_bundler::DynamicRequire;
use luck_config::{Editorconfigs, Project};
use luck_formatter::{FormatOptions, IndentStyle};
use luck_linter::{Category, LintConfig, RuleId, RuleLevel};
use luck_token::{LuaTarget, LuaVersion};

use crate::common::{load_error, loaded, project_with};

#[test]
fn defaults_take_every_lua_file_in_the_dialect_of_its_extension() {
    let project = Project::defaults(PathBuf::from("."));
    assert_eq!(project.target_for(Some(Path::new("a.lua"))), LuaTarget::Lua54);
    assert_eq!(project.target_for(Some(Path::new("a.luau"))), LuaTarget::Luau);
    assert_eq!(project.target_for(Some(Path::new("a"))), LuaTarget::Lua54);
    assert_eq!(project.target_for(None), LuaTarget::Lua54);
    assert!(project.filter().is_included(Path::new("src/a.lua")));
    assert_eq!(*project.linter_for(None).config(), LintConfig::default());
    assert_eq!(project.files(), Vec::<PathBuf>::new());
}

/// Extension and dialect are independent, so a Roblox or Rojo tree that
/// keeps Luau in `.lua` files can say so.
#[test]
fn either_key_names_any_dialect() {
    let (_roblox_directory, project) = loaded(&[("luck.json", r#"{ "lua": "roblox" }"#)]);
    assert_eq!(project.target_for(Some(Path::new("init.lua"))), LuaTarget::LuauRoblox);
    assert_eq!(project.target_for(Some(Path::new("init.luau"))), LuaTarget::Luau);

    let (_swapped_directory, swapped) =
        loaded(&[("luck.json", r#"{ "lua": "luau", "luau": "lua51" }"#)]);
    assert_eq!(swapped.target_for(Some(Path::new("a.lua"))), LuaTarget::Luau);
    assert_eq!(swapped.target_for(Some(Path::new("a.luau"))), LuaTarget::Lua51);
}

#[test]
fn rejects_a_dialect_on_its_value() {
    let text = r#"{ "luau": "lua99" }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert_eq!(diag.code, "E014");
    assert!(diag.message.contains("unknown variant `lua99`, expected one of"), "{diag:?}");
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""lua99""#[..]));
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

#[test]
fn an_overlay_wins_over_every_override() {
    let (directory, mut project) = loaded(&[(
        "luck.json",
        r#"{ "overrides": [{ "files": ["spec/**"], "lint": { "rules": { "shadowing": "warn" } } }] }"#,
    )]);
    let mut overlay = LintConfig::default();
    overlay.rules.set_level(RuleId::Shadowing, RuleLevel::Error);
    overlay.extra_globals.push("vim".to_owned());
    project.overlay_lint(&overlay);
    for relative in ["src/a.lua", "spec/a.lua"] {
        let linter = project.linter_for(Some(&directory.path().join(relative)));
        let lint = linter.config();
        assert_eq!(lint.rules.level(RuleId::Shadowing), Some(RuleLevel::Error), "{relative}");
        assert_eq!(lint.extra_globals, ["vim"], "{relative}");
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
    let canonical = |name: &str| directory.path().join(name).canonicalize().expect("exists");
    assert_eq!(project.files(), [canonical("luck.json"), canonical("base.json")]);
}

const LOVE: &str = r#"
[globals.love]
kind = "namespace"

[globals.love.members.draw]
kind = "function"
is_read_only = false
"#;

#[test]
fn std_definitions_layer_over_the_library_of_each_dialect() {
    let (directory, project) = loaded(&[
        ("std/love.toml", LOVE),
        ("luck.json", r#"{ "lua": "lua51", "luau": "roblox", "std": ["std/love.toml"] }"#),
    ]);
    for (file, target) in [("main.lua", LuaTarget::Lua51), ("main.luau", LuaTarget::LuauRoblox)] {
        let stdlib = project.stdlib_for(Some(Path::new(file)));
        assert_eq!(stdlib.target(), target);
        assert!(stdlib.library().lookup(["love", "draw"]).is_some(), "{file}");
        assert!(stdlib.library().lookup(["print"]).is_some(), "{file}");
    }
    let std_file = directory.path().join("std/love.toml").canonicalize().expect("exists");
    assert!(project.files().contains(&std_file), "{:?}", project.files());
}

#[test]
fn a_std_path_is_relative_to_the_file_that_names_it() {
    let (_directory, project) = loaded(&[
        ("shared/love.toml", LOVE),
        ("shared/base.json", r#"{ "std": ["love.toml"] }"#),
        ("luck.json", r#"{ "extends": ["shared/base.json"], "std": ["shared/love.toml"] }"#),
    ]);
    let stdlib = project.stdlib_for(None);
    assert!(stdlib.library().lookup(["love"]).is_some());
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
[globals.love]
kind = "namspace"
"#;
    let directory =
        project_with(&[("love.toml", definition), ("luck.json", r#"{ "std": ["love.toml"] }"#)]);
    let (path, diag) = load_error(&directory.path().join("luck.json"));
    assert!(path.ends_with("love.toml"), "{path:?}");
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
        project_with(&[("love.toml", definition), ("luck.json", r#"{ "std": ["love.toml"] }"#)]);
    let (path, diag) = load_error(&directory.path().join("luck.json"));
    assert!(path.ends_with("love.toml"), "{path:?}");
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
    assert_eq!(build.target, LuaTarget::Lua54);
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
fn bundle_options_carry_the_root_and_search_paths() {
    let (directory, project) = loaded(&[(
        "luck.json",
        r#"{ "search_paths": ["src/?.lua"], "dynamic_require": "error" }"#,
    )]);
    let options = project.bundle_options(LuaVersion::Lua54);
    assert_eq!(options.search_paths, ["src/?.lua"]);
    assert_eq!(options.dynamic_require, DynamicRequire::Error);
    assert_eq!(
        fs::canonicalize(options.project_root).expect("root exists"),
        directory.path().canonicalize().expect("exists")
    );
}
