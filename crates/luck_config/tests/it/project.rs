use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_file;
#[cfg(windows)]
use std::os::windows::fs::symlink_file;
use std::path::{Path, PathBuf};

use luck_bundler::DynamicRequire;
use luck_config::{ConfigError, DefinitionSource, Editorconfigs, Project};
use luck_formatter::{FormatOptions, IndentStyle};
use luck_linter::{Category, LintConfig, RuleId, RuleLevel};
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_stdlib::{SecurityLevel, StdlibEntry};

use crate::common::{load_error, loaded, project_with};

#[test]
fn defaults_take_every_lua_and_luau_file_as_luau() {
    let project = Project::defaults(PathBuf::from("."), FileSystemOs);
    assert!(!project.definitions().stdlib().library().has_roblox);
    assert_eq!(project.definitions().sources(), []);
    assert!(project.filter().is_included(Path::new("src/a.lua")));
    assert!(project.filter().is_included(Path::new("src/a.luau")));
    assert_eq!(*project.linter_for(None).config(), LintConfig::default());
    assert_eq!(project.files(), Vec::<PathBuf>::new());
}

#[test]
fn roblox_s_api_is_what_at_roblox_loads() {
    let (_roblox_directory, roblox) = loaded(&[("luck.json", r#"{ "definitions": ["@roblox"] }"#)]);
    assert!(roblox.definitions().stdlib().library().has_roblox);
    assert_eq!(roblox.definitions().sources(), [DefinitionSource::Roblox]);
    assert!(roblox.definitions().stdlib().library().lookup(["game"]).is_some());

    let (_luau_directory, luau) = loaded(&[("luck.json", "{}")]);
    assert!(!luau.definitions().stdlib().library().has_roblox);
    assert!(luau.definitions().stdlib().library().lookup(["game"]).is_none());
}

#[test]
fn rejects_a_definition_set_luck_lacks_on_its_entry() {
    let text = r#"{ "definitions": ["@roblox", "@lune"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert_eq!(diag.code, "E014");
    assert!(diag.message.contains("`definitions` names `@lune`"), "{diag:?}");
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""@lune""#[..]));
}

#[test]
fn the_variant_key_is_gone() {
    let directory = project_with(&[("luck.json", r#"{ "variant": "roblox" }"#)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert!(diag.message.contains("unknown field `variant`"), "{diag:?}");
}

#[test]
fn the_security_key_picks_the_level_of_a_roblox_projects_files() {
    let (_plugin_directory, plugin) =
        loaded(&[("luck.json", r#"{ "definitions": ["@roblox"], "security": "PluginSecurity" }"#)]);
    assert_eq!(plugin.definitions().security(), SecurityLevel::PluginSecurity);

    let (_script_directory, script) = loaded(&[("luck.json", r#"{ "definitions": ["@roblox"] }"#)]);
    assert_eq!(script.definitions().security(), SecurityLevel::None);
}

#[test]
fn rejects_security_on_a_project_without_roblox_s_api() {
    for text in [
        r#"{ "security": "None" }"#,
        r#"{ "definitions": ["app.d.luau"], "security": "PluginSecurity" }"#,
    ] {
        let directory = project_with(&[("app.d.luau", ""), ("luck.json", text)]);
        let (_, diag) = load_error(&directory.path().join("luck.json"));
        assert_eq!(diag.code, "E014");
        assert!(
            diag.message.contains("`security` applies only to a project that loads `@roblox`"),
            "{text}: {diag:?}"
        );
    }
}

#[test]
fn rejects_a_security_level_no_script_runs_at_on_its_value() {
    let text = r#"{ "definitions": ["@roblox"], "security": "RobloxSecurity" }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert_eq!(diag.code, "E014");
    assert!(
        diag.message.contains(
            "unknown variant `RobloxSecurity`, expected one of `None`, `LocalUserSecurity`, \
             `PluginSecurity`, `RobloxScriptSecurity`"
        ),
        "{diag:?}"
    );
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""RobloxSecurity""#[..]));
}

#[test]
fn rejects_entry_beside_entries() {
    let directory = project_with(&[(
        "luck.json",
        r#"{ "entry": "a.luau", "entries": [{ "entry": "b.luau", "output": "c.luau" }] }"#,
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

    let base = linter_at("src/a.luau");
    assert_eq!(base.config().rules.level(RuleId::Shadowing), Some(RuleLevel::Warn));

    let spec = linter_at("spec/a.luau");
    assert_eq!(spec.config().rules.level(RuleId::Shadowing), Some(RuleLevel::Off));
    assert_eq!(spec.config().extra_globals, ["a", "it"]);
    assert_eq!(spec.config().categories.level(Category::Style), None);

    let slow = linter_at("spec/slow/a.luau");
    assert_eq!(slow.config().rules.level(RuleId::Shadowing), Some(RuleLevel::Off));
    assert_eq!(slow.config().categories.level(Category::Style), Some(RuleLevel::Error));

    let fixture = linter_at("spec/fixtures/a.luau");
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
    let linter = project.linter_for(Some(&directory.path().join("spec/a.luau")));
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
    let target = elsewhere.path().join("b.luau");
    fs::write(&target, "").expect("write b.luau");
    fs::create_dir_all(directory.path().join("src")).expect("mkdir src");
    let link = directory.path().join("src/b.luau");
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
    for relative in ["src/a.luau", "spec/a.luau"] {
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
[*.luau]
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

    let source = options_at("src/a.luau");
    assert_eq!(source.indent_style, IndentStyle::Spaces);
    assert_eq!(source.indent_width, 3);
    assert_eq!(source.line_width, 90);
    assert_eq!(options_at("spec/a.luau").line_width, 80);

    let without_editorconfig = project.format_options(Some(&directory.path().join("a.luau")), None);
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

const APP: &str = "declare app: { onMessage: (text: string) -> () }";

#[test]
fn definitions_layer_in_order_over_luau_s_library() {
    let (directory, project) = loaded(&[
        ("types/app.d.luau", APP),
        (
            "luck.json",
            r#"{ "security": "PluginSecurity", "definitions": ["@roblox", "types/app.d.luau"] }"#,
        ),
    ]);
    let library = project.definitions().stdlib().library();
    assert!(library.has_roblox);
    assert!(library.lookup(["app", "onMessage"]).is_some());
    assert!(library.lookup(["print"]).is_some());
    assert!(library.shape_member("Instance", "GetDebugId").is_some(), "a plugin's level");
    let file =
        FileSystemOs.canonicalize(&directory.path().join("types/app.d.luau")).expect("exists");
    assert!(project.files().contains(&file), "{:?}", project.files());
    assert!(project.filter().is_included(&file), "a definition file is one of the project's");
    assert_eq!(
        project.definitions().sources(),
        [DefinitionSource::Roblox, DefinitionSource::File { path: file, text: APP.to_owned() }]
    );
}

#[test]
fn a_definitions_path_is_relative_to_the_file_that_names_it_and_extended_ones_come_first() {
    let (directory, project) = loaded(&[
        ("shared/app.d.luau", APP),
        ("shared/base.json", r#"{ "definitions": ["app.d.luau"] }"#),
        ("local.d.luau", "declare app: { onMessage: (text: string, sender: string) -> () }"),
        (
            "luck.json",
            r#"{ "extends": ["shared/base.json"], "definitions": ["shared/app.d.luau", "local.d.luau"] }"#,
        ),
    ]);
    let canonical =
        |name: &str| FileSystemOs.canonicalize(&directory.path().join(name)).expect("exists");
    let paths: Vec<&PathBuf> = project
        .definitions()
        .sources()
        .iter()
        .filter_map(|source| match source {
            DefinitionSource::File { path, .. } => Some(path),
            DefinitionSource::Roblox => None,
        })
        .collect();
    assert_eq!(paths, [&canonical("shared/app.d.luau"), &canonical("local.d.luau")]);
    let Some(StdlibEntry::Function(on_message)) =
        project.definitions().stdlib().library().lookup(["app", "onMessage"])
    else {
        panic!("app.onMessage should be a function");
    };
    assert_eq!(on_message.min_args(), 2, "the later file's declaration replaces the earlier's");
}

#[test]
fn a_definitions_file_that_cannot_be_read_is_reported_on_its_name() {
    let text = r#"{ "definitions": ["gone.d.luau"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (path, diag) = load_error(&directory.path().join("luck.json"));
    assert!(path.ends_with("luck.json"), "{path:?}");
    let message = &diag.message;
    assert!(message.contains("`definitions` names `gone.d.luau`, which can't be read"), "{diag:?}");
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""gone.d.luau""#[..]));
}

#[test]
fn a_definitions_file_that_does_not_parse_adds_nothing_to_its_project() {
    let broken = "declare function greet(name: string: string";
    let (directory, project) = loaded(&[
        ("app.d.luau", broken),
        (
            "luck.json",
            r#"{ "definitions": ["@roblox", "app.d.luau"], "lint": { "extra_globals": ["it"] } }"#,
        ),
    ]);
    let path = FileSystemOs.canonicalize(&directory.path().join("app.d.luau")).expect("exists");
    let file = DefinitionSource::File { path: path.clone(), text: broken.to_owned() };
    assert_eq!(project.definitions().sources(), [DefinitionSource::Roblox, file]);
    assert!(project.files().contains(&path), "{:?}", project.files());
    let library = project.definitions().stdlib().library();
    assert!(library.has_roblox);
    assert!(library.lookup(["greet"]).is_none());
    assert_eq!(project.linter_for(None).config().extra_globals, ["it"]);
}

#[test]
fn a_definitions_file_whose_name_does_not_end_in_d_luau_is_reported_on_its_name() {
    let text = r#"{ "definitions": ["app.luau"] }"#;
    let directory = project_with(&[("app.luau", APP), ("luck.json", text)]);
    let (path, diag) = load_error(&directory.path().join("luck.json"));
    assert!(path.ends_with("luck.json"), "{path:?}");
    assert_eq!(
        diag.message,
        "invalid config: `definitions` names `app.luau`, but a definition file's name ends in \
         `.d.luau`"
    );
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""app.luau""#[..]));
}

#[test]
fn a_given_config_reads_its_definitions_from_its_root() {
    let directory = project_with(&[("types/app.d.luau", APP)]);
    let config = r#"{ "definitions": ["@roblox", "types/app.d.luau"], "lint": { "extra_globals": ["it"] } }"#;
    let project = Project::from_config(config, directory.path().to_path_buf(), FileSystemOs)
        .expect("the config loads");
    let library = project.definitions().stdlib().library();
    assert!(library.has_roblox);
    assert!(library.lookup(["app", "onMessage"]).is_some());
    assert_eq!(project.root(), directory.path());
    assert_eq!(project.linter_for(None).config().extra_globals, ["it"]);
}

#[test]
fn a_given_config_reports_what_a_luck_json_with_its_text_does() {
    let text = r#"{ "definitions": ["@lune"] }"#;
    let directory = project_with(&[("luck.json", text)]);
    let (_, from_file) = load_error(&directory.path().join("luck.json"));
    let given = Project::from_config(text, directory.path().to_path_buf(), FileSystemOs)
        .expect_err("the config doesn't load");
    assert_eq!(given, ConfigError::Given(from_file));
}

#[test]
fn a_given_config_extends_nothing() {
    let text = r#"{ "extends": ["base.json"] }"#;
    let directory = project_with(&[("base.json", "{}")]);
    let Err(ConfigError::Given(diag)) =
        Project::from_config(text, directory.path().to_path_buf(), FileSystemOs)
    else {
        panic!("the config doesn't load");
    };
    assert!(diag.message.starts_with("invalid config: `extends` names `base.json`"), "{diag:?}");
    assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&br#""base.json""#[..]));
}

#[test]
fn the_std_key_is_gone() {
    let directory = project_with(&[("luck.json", r#"{ "std": ["app.toml"] }"#)]);
    let (_, diag) = load_error(&directory.path().join("luck.json"));
    assert!(diag.message.contains("unknown field `std`"), "{diag:?}");
}

#[test]
fn builds_resolve_entries_profiles_and_outputs() {
    let (directory, project) = loaded(&[
        ("src/main.luau", "return 1"),
        (
            "luck.json",
            r#"{
                "entry": "src/main.luau", "output_dir": "dist", "minify": false,
                "profiles": { "release": { "minify": true } }
            }"#,
        ),
    ]);
    let builds = project.builds(Some("release")).expect("builds");
    let [build] = builds.as_slice() else { panic!("one build: {builds:?}") };
    assert!(build.minify);
    assert_eq!(build.output, directory.path().join("dist").join("main.luau"));
    let error = project.builds(Some("nightly")).unwrap_err();
    assert_eq!(error, "profile `nightly` isn't defined in `luck.json`");
}

#[test]
fn rejects_an_output_that_overwrites_the_entry() {
    let (_directory, project) = loaded(&[
        ("main.luau", "return 1"),
        ("luck.json", r#"{ "entry": "main.luau", "output_dir": "." }"#),
    ]);
    let builds = project.builds(None);
    assert!(builds.is_err(), "a bundle written over its own entry destroys it: {builds:?}");
}

#[test]
fn rejects_two_entries_writing_the_same_output() {
    let (_directory, project) = loaded(&[
        ("a.luau", "return 1"),
        ("b.luau", "return 2"),
        (
            "luck.json",
            r#"{ "entries": [
                { "entry": "a.luau", "output": "bundle.luau" },
                { "entry": "b.luau", "output": "bundle.luau" }
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
