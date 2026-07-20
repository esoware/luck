//! `luck.json` project configuration: the typed schema, its `extends`/profile
//! merge semantics, file loading and discovery, build-target resolution, and
//! project file filtering.
//!
//! The four concerns live in focused submodules and are re-exported here, so
//! the whole surface remains reachable as `luck_core::config::*`.

mod filter;
mod load;
mod resolve;
mod schema;

pub use filter::ProjectFilter;
pub use load::{LuauRc, discover_config, load_with_extends, parse_luaurc, parse_luck_config};
pub use resolve::{BuildConfig, DEFAULT_SEARCH_PATHS, resolve_build_config};
pub use schema::{
    EntryConfig, FormatConfig, LintConfig, LuckConfig, ProfileOverrides, RuleSetting,
};

pub(crate) use schema::merge_format;

pub(crate) const CONFIG_FILE_NAME: &str = "luck.json";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{Category, DiagnosticSeverity};
    use crate::format_options::{
        BlockNewlineGaps, CallParentheses, IndentStyle, QuoteStyle, SpaceAfterFunction,
    };
    use crate::types::LuaTarget;
    use std::path::Path;

    /// Path to the committed schema, relative to this crate's manifest.
    const COMMITTED_SCHEMA_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/vscode/schemas/luckrc.schema.json"
    );

    /// Single source of truth for the schema's serialized form. Both the
    /// regenerator and the up-to-date checker call this so they cannot disagree.
    fn luckrc_schema_json() -> String {
        let schema = schemars::schema_for!(LuckConfig);
        let mut pretty = serde_json::to_string_pretty(&schema).expect("schema serializes");
        pretty.push('\n');
        pretty
    }

    #[test]
    #[ignore = "writes the committed schema; run with --ignored to refresh"]
    fn regenerate_luckrc_schema() {
        std::fs::write(COMMITTED_SCHEMA_PATH, luckrc_schema_json()).expect("write schema");
    }

    #[test]
    fn luckrc_schema_is_up_to_date() {
        let committed = std::fs::read_to_string(COMMITTED_SCHEMA_PATH).expect("read schema");
        assert_eq!(
            committed,
            luckrc_schema_json(),
            "luckrc.schema.json is stale; regenerate it with \
             `cargo test -p luck_core regenerate_luckrc_schema -- --ignored`"
        );
    }

    #[test]
    fn test_parse_luck_config() {
        let json = r#"{
            "lua": "lua54",
            "entry": "src/main.lua",
            "output_dir": "dist",
            "search_paths": ["src/?.lua", "lib/?.lua"]
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.entry.as_deref(), Some("src/main.lua"));
        assert_eq!(config.output_dir.as_deref(), Some("dist"));
        assert_eq!(
            config.search_paths.as_deref(),
            Some(&["src/?.lua".to_string(), "lib/?.lua".to_string()][..])
        );
    }

    #[test]
    fn test_parse_luck_config_minimal() {
        let json = r#"{"luau": "luau", "entry": "main.luau", "output_dir": "out"}"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert_eq!(config.entry.as_deref(), Some("main.luau"));
        assert!(config.search_paths.is_none());
        assert!(config.minify.is_none());
    }

    #[test]
    fn test_parse_luaurc() {
        let json5 = r#"{
            // This is a comment
            "languageMode": "strict",
            "aliases": {
                "utils": "../shared/utils",
                "components": "./components",
            },  // trailing comma
        }"#;
        let rc = parse_luaurc(json5).expect("deserialize failed");
        assert_eq!(rc.language_mode.as_deref(), Some("strict"));
        let aliases = rc.aliases.expect("deserialize failed");
        assert_eq!(
            aliases.get("utils").map(|s| s.as_str()),
            Some("../shared/utils")
        );
        assert_eq!(
            aliases.get("components").map(|s| s.as_str()),
            Some("./components")
        );
    }

    #[test]
    fn test_parse_luaurc_empty() {
        let json5 = "{}";
        let rc = parse_luaurc(json5).expect("deserialize failed");
        assert!(rc.aliases.is_none());
        assert!(rc.language_mode.is_none());
    }

    #[test]
    fn test_parse_entries_config() {
        let json = r#"{
            "lua": "lua54",
            "entries": [
                {"entry": "src/main.lua", "output": "dist/main.lua"},
                {"entry": "src/other.lua", "output": "dist/other.lua"}
            ]
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        let entries = config.entries.expect("deserialize failed");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry, "src/main.lua");
        assert_eq!(entries[0].output, "dist/main.lua");
    }

    #[test]
    fn test_parse_profiles_config() {
        let json = r#"{
            "lua": "lua54",
            "entry": "src/main.lua",
            "output_dir": "dist",
            "profiles": {
                "release": {
                    "minify": true
                }
            }
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        let profiles = config.profiles.expect("deserialize failed");
        let release = profiles.get("release").expect("deserialize failed");
        assert_eq!(release.minify, Some(true));
    }

    #[test]
    fn test_parse_output_field() {
        let json = r#"{
            "lua": "lua54",
            "entry": "src/main.lua",
            "output": "dist/bundle.lua"
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.output.as_deref(), Some("dist/bundle.lua"));
    }

    #[test]
    fn test_missing_optional_fields_default_to_none() {
        let json = r#"{"lua": "lua51"}"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.lua.as_deref(), Some("lua51"));
        assert!(config.entry.is_none());
        assert!(config.output.is_none());
        assert!(config.output_dir.is_none());
        assert!(config.entries.is_none());
        assert!(config.minify.is_none());
        assert!(config.search_paths.is_none());
        assert!(config.transforms.is_none());
        assert!(config.profiles.is_none());
        assert!(config.preamble.is_none());
        assert!(config.luck_preamble.is_none());
        assert!(config.lint.is_none());
    }

    #[test]
    fn test_parse_lint_config() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "lint": {
                "rule_overrides": {
                    "unused_variable": { "enabled": false },
                    "undefined_variable": { "severity": "error" }
                },
                "extra_globals": ["vim", "roblox"],
                "restricted_module_paths": ["secret.path"],
                "max_cyclomatic_complexity": 15,
                "disable_default_rules": true
            }
        }"#;
        let config = parse_luck_config(json).expect("deserialize");
        let lint = config.lint.expect("lint present");
        assert_eq!(
            lint.extra_globals,
            vec!["vim".to_string(), "roblox".to_string()]
        );
        assert_eq!(
            lint.restricted_module_paths,
            vec!["secret.path".to_string()]
        );
        assert_eq!(lint.max_cyclomatic_complexity, Some(15));
        assert!(lint.disable_default_rules);
        let uv = lint
            .rule_overrides
            .get("unused_variable")
            .expect("unused_variable override");
        assert_eq!(uv.enabled, Some(false));
        assert!(uv.severity.is_none());
        let udef = lint
            .rule_overrides
            .get("undefined_variable")
            .expect("undefined_variable override");
        assert_eq!(udef.severity, Some(DiagnosticSeverity::Error));
    }

    #[test]
    fn test_parse_lint_config_defaults_when_omitted() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "lint": {}
        }"#;
        let config = parse_luck_config(json).expect("deserialize");
        let lint = config.lint.expect("lint present");
        assert!(lint.rule_overrides.is_empty());
        assert!(lint.extra_globals.is_empty());
        assert!(lint.restricted_module_paths.is_empty());
        assert!(lint.max_cyclomatic_complexity.is_none());
        assert!(!lint.disable_default_rules);
    }

    #[test]
    fn test_invalid_json5_returns_error() {
        let bad_json = r#"{ target: }"#;
        let result = parse_luck_config(bad_json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("Failed to parse"),
            "error should mention parsing failure: {err}"
        );
    }

    #[test]
    fn test_completely_invalid_input() {
        let result = parse_luck_config("not json at all !!!");
        assert!(result.is_err());
    }

    #[test]
    fn unknown_keys_rejected() {
        // `deny_unknown_fields` makes any unrecognized top-level key a hard error.
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "unknown_key": true,
            "also_unknown": 42
        }"#;
        let result = parse_luck_config(json);
        assert!(result.is_err(), "unknown top-level keys must error");
        let err = result.unwrap_err();
        assert!(
            err.contains("unknown_key") || err.contains("unknown field"),
            "error should name the offending key: {err}"
        );
    }

    #[test]
    fn unknown_nested_key_rejected() {
        // `deny_unknown_fields` propagates into nested config structs.
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "format": {
                "line_width": 100,
                "not_a_real_option": true
            }
        }"#;
        let result = parse_luck_config(json);
        assert!(result.is_err(), "unknown nested keys must error");
        let err = result.unwrap_err();
        assert!(
            err.contains("not_a_real_option") || err.contains("unknown field"),
            "error should name the offending nested key: {err}"
        );
    }

    #[test]
    fn invalid_enum_value_rejected() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "format": {
                "quote_style": "bogus"
            }
        }"#;
        let result = parse_luck_config(json);
        assert!(result.is_err(), "invalid enum values must error");
        let err = result.unwrap_err();
        assert!(
            err.contains("bogus") || err.contains("unknown variant"),
            "error should name the offending value: {err}"
        );
    }

    #[test]
    fn invalid_severity_value_rejected() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "lint": {
                "rule_overrides": {
                    "undefined_variable": { "severity": "fatal" }
                }
            }
        }"#;
        let result = parse_luck_config(json);
        assert!(result.is_err(), "invalid severity values must error");
        let err = result.unwrap_err();
        assert!(
            err.contains("fatal") || err.contains("unknown variant"),
            "error should name the offending value: {err}"
        );
    }

    #[test]
    fn test_profile_override_with_transforms() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "minify": false,
            "transforms": {
                "fold_constants": true,
                "rename_locals": false
            },
            "profiles": {
                "release": {
                    "minify": true,
                    "transforms": {
                        "fold_constants": false,
                        "rename_locals": true
                    }
                }
            }
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");

        assert_eq!(config.minify, Some(false));
        let base_transforms = config
            .transforms
            .as_ref()
            .expect("test config has transforms");
        assert!(base_transforms.fold_constants);
        assert!(!base_transforms.rename_locals);

        let profiles = config.profiles.as_ref().expect("test config has profiles");
        let release = profiles
            .get("release")
            .expect("test config has release profile");
        assert_eq!(release.minify, Some(true));
        let release_transforms = release
            .transforms
            .as_ref()
            .expect("release profile has transforms");
        assert!(!release_transforms.fold_constants);
        assert!(release_transforms.rename_locals);
    }

    #[test]
    fn test_json5_features_supported() {
        // JSON5 allows comments, trailing commas, unquoted keys
        let json5 = r#"{
            // this is a comment
            lua: "lua54",
            entry: "main.lua",
            output_dir: "dist",
            minify: true, // trailing comma
        }"#;
        let config = parse_luck_config(json5).expect("JSON5 features should parse");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.minify, Some(true));
    }

    #[test]
    fn test_parse_format_config() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "format": {
                "line_width": 120,
                "indent_style": "spaces",
                "indent_width": 4,
                "quote_style": "double"
            }
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        let format = config.format.expect("format should be present");
        assert_eq!(format.line_width, Some(120));
        assert_eq!(format.indent_style, Some(IndentStyle::Spaces));
        assert_eq!(format.indent_width, Some(4));
        assert_eq!(format.quote_style, Some(QuoteStyle::Double));
    }

    #[test]
    fn test_parse_luaurc_invalid_json() {
        let result = parse_luaurc("not valid json {{{");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to parse .luaurc"));
    }

    #[test]
    fn test_parse_config_entry_and_entries_both_present() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "entries": [{"entry": "a.lua", "output": "b.lua"}],
            "output_dir": "dist"
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert!(config.entry.is_some());
        assert!(config.entries.is_some());
    }

    #[test]
    fn test_preamble_and_luck_preamble() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "preamble": "-- Generated by luck",
            "luck_preamble": true
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.preamble.as_deref(), Some("-- Generated by luck"));
        assert_eq!(config.luck_preamble, Some(true));
    }

    #[test]
    fn test_parse_format_polish_options() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "format": {
                "block_newline_gaps": "preserve",
                "sort_requires": true,
                "space_after_function_names": "calls",
                "magic_trailing_comma": true,
                "call_parentheses": "input"
            }
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        let format = config.format.expect("format should be present");
        assert_eq!(format.block_newline_gaps, Some(BlockNewlineGaps::Preserve));
        assert_eq!(format.sort_requires, Some(true));
        assert_eq!(
            format.space_after_function_names,
            Some(SpaceAfterFunction::Calls)
        );
        assert_eq!(format.magic_trailing_comma, Some(true));
        assert_eq!(format.call_parentheses, Some(CallParentheses::Input));
    }

    #[test]
    fn test_format_polish_defaults_when_omitted() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "format": {}
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        let format = config.format.expect("format should be present");
        assert!(format.block_newline_gaps.is_none());
        assert!(format.sort_requires.is_none());
        assert!(format.space_after_function_names.is_none());
        assert!(format.magic_trailing_comma.is_none());
    }

    #[test]
    fn parses_lua_luau_and_scope_keys() {
        let json = r#"{ "lua":"lua53","luau":"roblox","root":true,
            "extends":["base.json"],"include":["src/**/*.lua"],"exclude":["**/gen/**"] }"#;
        let config = parse_luck_config(json).expect("parse");
        assert_eq!(config.lua.as_deref(), Some("lua53"));
        assert_eq!(config.luau.as_deref(), Some("roblox"));
        assert_eq!(config.root, Some(true));
        assert_eq!(config.extends, Some(vec!["base.json".to_string()]));
        assert_eq!(config.include.as_deref().unwrap().len(), 1);
        assert_eq!(config.exclude.as_deref().unwrap().len(), 1);
    }

    #[test]
    fn target_resolution_by_extension() {
        let c = parse_luck_config(r#"{ "lua":"lua53","luau":"roblox" }"#).unwrap();
        assert_eq!(
            c.target_for_path(Path::new("a.lua")).unwrap(),
            LuaTarget::Lua53
        );
        assert_eq!(
            c.target_for_path(Path::new("a.luau")).unwrap(),
            LuaTarget::LuauRoblox
        );
    }

    #[test]
    fn target_defaults_and_validation() {
        let c = parse_luck_config("{}").unwrap();
        assert_eq!(
            c.target_for_path(Path::new("a.lua")).unwrap(),
            LuaTarget::Lua54
        );
        assert_eq!(
            c.target_for_path(Path::new("a.luau")).unwrap(),
            LuaTarget::Luau
        );
        assert!(
            parse_luck_config(r#"{ "lua":"luau" }"#)
                .unwrap()
                .lua_target()
                .is_err()
        );
        assert!(
            parse_luck_config(r#"{ "luau":"lua54" }"#)
                .unwrap()
                .luau_target()
                .is_err()
        );
    }

    #[test]
    fn extends_merge_semantics() {
        let base = parse_luck_config(
            r#"{ "lua":"lua54","lint":{"extra_globals":["a"],
            "rule_overrides":{"x":{"enabled":false}}} }"#,
        )
        .unwrap();
        let child = parse_luck_config(
            r#"{ "lint":{"extra_globals":["b"],
            "rule_overrides":{"y":{"severity":"error"}}} }"#,
        )
        .unwrap();
        let merged = child.merge_onto(base);
        let lint = merged.lint.unwrap();
        assert_eq!(lint.extra_globals, vec!["a".to_string(), "b".to_string()]);
        assert!(lint.rule_overrides.contains_key("x") && lint.rule_overrides.contains_key("y"));
        assert_eq!(merged.lua.as_deref(), Some("lua54"));
    }

    #[test]
    fn merge_lint_categories_complexity_and_lockdown() {
        // categories concat (base then child), max_cyclomatic_complexity is
        // child-or-base, and disable_default_rules is ORed so a locked-down
        // base stays locked down when the child omits the flag.
        let base = parse_luck_config(
            r#"{ "lua":"lua54","lint":{"categories":["style"],
            "max_cyclomatic_complexity":10,"disable_default_rules":true}}"#,
        )
        .unwrap();
        let child = parse_luck_config(r#"{ "lint":{"categories":["performance"]} }"#).unwrap();
        let lint = child.merge_onto(base).lint.unwrap();
        assert_eq!(
            lint.categories,
            vec![Category::Style, Category::Performance]
        );
        assert_eq!(lint.max_cyclomatic_complexity, Some(10));
        assert!(lint.disable_default_rules);
    }

    #[test]
    fn category_perf_alias_parses() {
        let config = parse_luck_config(r#"{ "lint": { "categories": ["perf"] } }"#).unwrap();
        assert_eq!(config.lint.unwrap().categories, vec![Category::Performance]);
    }

    #[test]
    fn extends_cycle_errors() {
        // Build two temp files that extend each other; load_with_extends must Err.
        let dir = std::env::temp_dir().join(format!("luck_extends_cycle_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.json");
        let b = dir.join("b.json");
        std::fs::write(&a, r#"{ "extends": ["b.json"], "lua": "lua54" }"#).unwrap();
        std::fs::write(&b, r#"{ "extends": ["a.json"] }"#).unwrap();
        assert!(load_with_extends(&a).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn root_extends_within_boundary_ok() {
        let dir = std::env::temp_dir().join(format!("luck_root_within_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("luck.json");
        let child = dir.join("base.json");
        std::fs::write(
            &root,
            r#"{ "root": true, "extends": ["base.json"], "lua": "lua54" }"#,
        )
        .unwrap();
        std::fs::write(&child, r#"{ "lint": { "extra_globals": ["a"] } }"#).unwrap();
        let config = load_with_extends(&root).expect("within-boundary extends should load");
        assert_eq!(config.lua.as_deref(), Some("lua54"));
        assert_eq!(config.root, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn root_extends_escaping_boundary_errors() {
        let base = std::env::temp_dir().join(format!("luck_root_escape_{}", std::process::id()));
        let inner = base.join("project");
        std::fs::create_dir_all(&inner).unwrap();
        let outside = base.join("outside.json");
        let root = inner.join("luck.json");
        std::fs::write(&outside, r#"{ "lua": "lua53" }"#).unwrap();
        std::fs::write(&root, r#"{ "root": true, "extends": ["../outside.json"] }"#).unwrap();
        let result = load_with_extends(&root);
        assert!(result.is_err(), "escaping extends must error");
        let message = result.unwrap_err();
        assert!(
            message.contains("escapes the root boundary"),
            "unexpected error: {message}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn non_root_extends_parent_ok() {
        let base = std::env::temp_dir().join(format!("luck_nonroot_parent_{}", std::process::id()));
        let inner = base.join("project");
        std::fs::create_dir_all(&inner).unwrap();
        let parent = base.join("shared.json");
        let child = inner.join("luck.json");
        std::fs::write(&parent, r#"{ "lua": "lua52" }"#).unwrap();
        std::fs::write(&child, r#"{ "extends": ["../shared.json"] }"#).unwrap();
        let config =
            load_with_extends(&child).expect("non-root config may extend a parent-dir config");
        assert_eq!(config.lua.as_deref(), Some("lua52"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
