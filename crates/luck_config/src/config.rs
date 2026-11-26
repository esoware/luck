//! One `luck.json` as written, before `extends` and validation. Nothing here
//! touches the filesystem.

use std::fmt;
use std::path::PathBuf;

use luck_bundler::DynamicRequire;
use luck_formatter::FormatConfig;
use luck_linter::LintConfig;
use luck_minifier::TransformConfig;
use luck_stdlib::{SecurityLevel, Variant};
use rustc_hash::FxHashMap;
use serde::de::{Deserialize, Deserializer, Error, SeqAccess, Visitor};

use crate::glob::Glob;

/// One entry of a multi-entry build: the file to bundle and where to write
/// the bundle, both relative to the `luck.json` directory.
#[derive(Debug, Clone, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub(crate) struct EntryConfig {
    pub entry: String,
    pub output: String,
}

/// A build profile, selected with `luck build --profile <name>`.
///
/// Its `minify` and `transforms` replace the top-level ones. `--release` and
/// `--dev` select the `release` and `dev` profiles.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileOverrides {
    pub minify: Option<bool>,
    pub transforms: Option<TransformConfig>,
}

/// Settings for the files some globs match, over the rest of the config.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub(crate) struct OverrideConfig {
    /// Globs of the files this applies to, matched as `include` is.
    #[cfg_attr(test, schemars(with = "Vec<String>"))]
    pub files: Vec<Glob>,
    /// Globs of files this leaves alone, even when `files` takes them.
    #[serde(default)]
    #[cfg_attr(test, schemars(with = "Vec<String>"))]
    pub exclude_files: Vec<Glob>,
    /// Formatter settings over the `format` section's.
    pub format: Option<FormatConfig>,
    /// Linter settings over the `lint` section's, which merge as `extends`
    /// merges them.
    pub lint: Option<LintConfig>,
}

/// Top-level luck.json configuration.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub(crate) struct LuckConfig {
    /// Where every `.luau` and `.lua` file runs (default `standalone`):
    /// `"standalone"` for standalone Luau, or `"roblox"` for Roblox, whose globals,
    /// services, and enums join Luau's standard library.
    pub variant: Option<Variant>,
    /// The security level a Roblox project's files run at (default `None`),
    /// which decides the part of Roblox's API `luck check` gives them:
    /// `"None"` for a `Script` or `LocalScript`, `"LocalUserSecurity"`,
    /// `"PluginSecurity"` for a plugin or Studio's command bar, or
    /// `"RobloxScriptSecurity"` for a `CoreScript`. Only a `"roblox"` project
    /// takes it.
    pub security: Option<SecurityLevel>,
    /// Std definition files, relative to this file's directory, that layer a
    /// host's globals, fields, and functions over the variant's standard
    /// library, in order. Each is TOML in the format of luck's own
    /// `stdlib_data`, which
    /// <https://github.com/esoware/luck/tree/main/crates/luck_stdlib#std-definitions>
    /// describes.
    #[cfg_attr(test, schemars(with = "Option<Vec<String>>"))]
    pub std: Option<Vec<PathBuf>>,
    /// Keeps the `extends` chain inside this file's directory.
    pub root: Option<bool>,
    /// Configs this one builds on, relative to its directory. A later one
    /// overrides an earlier one, and this file overrides them all.
    pub extends: Option<Vec<String>>,
    /// Globs of the files `lint`, `fmt`, and `check` take, relative to the
    /// `luck.json` directory (default every `.luau` and `.lua` file). `*`
    /// stops at `/`, and `**/` crosses directories.
    #[cfg_attr(test, schemars(with = "Option<Vec<String>>"))]
    pub include: Option<Vec<Glob>>,
    /// Globs of files to leave out, even when `include` takes them.
    #[cfg_attr(test, schemars(with = "Option<Vec<String>>"))]
    pub exclude: Option<Vec<Glob>>,
    /// The file `luck build` bundles, relative to the `luck.json` directory.
    pub entry: Option<String>,
    /// The directory `luck build` writes the bundle of `entry` to, under the
    /// entry's file name.
    pub output_dir: Option<String>,
    /// The file `luck build` writes the bundle of `entry` to, in place of one
    /// in `output_dir`.
    pub output: Option<String>,
    /// Several builds in place of `entry`, each with its own output.
    #[cfg_attr(test, schemars(with = "Option<Vec<EntryConfig>>", length(min = 1)))]
    pub entries: Option<Entries>,
    /// Whether `luck build` minifies its bundles (default `false`).
    pub minify: Option<bool>,
    /// How the bundler treats a `require` whose argument isn't a string
    /// literal: `"error"`, `"warn"` (default), or `"allow"`.
    pub dynamic_require: Option<DynamicRequire>,
    pub transforms: Option<TransformConfig>,
    pub profiles: Option<FxHashMap<String, ProfileOverrides>>,
    /// A comment `luck build` writes at the top of each bundle.
    pub preamble: Option<String>,
    /// Whether `luck build` names luck and its version at the top of each
    /// bundle (default `false`).
    pub luck_preamble: Option<bool>,
    pub format: Option<FormatConfig>,
    pub lint: Option<LintConfig>,
    /// Settings for the files some globs match, each over the ones before it.
    /// An `extends` chain lists the overrides of the files it extends first,
    /// and every glob matches relative to the `luck.json` directory.
    pub overrides: Option<Vec<OverrideConfig>>,
}

impl LuckConfig {
    /// `self` layered on `base`, as a file is on the files it `extends`.
    ///
    /// `format` and `lint` merge field by field. `std`, `include`, `exclude`,
    /// and `overrides` list `base`'s items and then `self`'s, without an item
    /// that comes again later. Every other key, `entries`, `profiles`, and
    /// `transforms` among them, comes whole from `self` when set there and
    /// from `base` otherwise.
    pub(crate) fn merge_onto(self, base: Self) -> Self {
        Self {
            variant: self.variant.or(base.variant),
            security: self.security.or(base.security),
            std: merge_with(self.std, base.std, layer),
            root: self.root.or(base.root),
            extends: None,
            include: merge_with(self.include, base.include, layer),
            exclude: merge_with(self.exclude, base.exclude, layer),
            entry: self.entry.or(base.entry),
            output_dir: self.output_dir.or(base.output_dir),
            output: self.output.or(base.output),
            entries: self.entries.or(base.entries),
            minify: self.minify.or(base.minify),
            dynamic_require: self.dynamic_require.or(base.dynamic_require),
            transforms: self.transforms.or(base.transforms),
            profiles: self.profiles.or(base.profiles),
            preamble: self.preamble.or(base.preamble),
            luck_preamble: self.luck_preamble.or(base.luck_preamble),
            format: merge_with(self.format, base.format, FormatConfig::merge_onto),
            lint: merge_with(self.lint, base.lint, LintConfig::merge_onto),
            overrides: merge_with(self.overrides, base.overrides, layer),
        }
    }
}

fn merge_with<T, Merge>(child: Option<T>, base: Option<T>, merge: Merge) -> Option<T>
where
    Merge: FnOnce(T, T) -> T,
{
    match (child, base) {
        (Some(child), Some(base)) => Some(merge(child, base)),
        (child, base) => child.or(base),
    }
}

/// `base` and then `child`, without an item of `base` that `child` repeats,
/// so a file reached twice through `extends` doesn't double a list, and an
/// order the child gives wins.
fn layer<T>(child: Vec<T>, mut base: Vec<T>) -> Vec<T>
where
    T: PartialEq,
{
    base.retain(|item| !child.contains(item));
    base.extend(child);
    base
}

/// The `entries` of a multi-entry build, at least one.
#[derive(Debug, Clone)]
pub(crate) struct Entries(pub Vec<EntryConfig>);

// The error comes from inside the list, so a format that tracks positions
// places it on the list.
impl<'de> Deserialize<'de> for Entries {
    fn deserialize<Source>(deserializer: Source) -> Result<Self, Source::Error>
    where
        Source: Deserializer<'de>,
    {
        deserializer.deserialize_seq(EntriesVisitor).map(Self)
    }
}

struct EntriesVisitor;

impl<'de> Visitor<'de> for EntriesVisitor {
    type Value = Vec<EntryConfig>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a list of entries")
    }

    fn visit_seq<Access>(self, mut seq: Access) -> Result<Self::Value, Access::Error>
    where
        Access: SeqAccess<'de>,
    {
        let mut entries = Vec::new();
        while let Some(entry) = seq.next_element()? {
            entries.push(entry);
        }
        if entries.is_empty() {
            return Err(Error::custom("`entries` is empty, so list at least one entry to build"));
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_formatter::{
        BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, HexCase, IndentStyle,
        LineEndings, QuoteStyle, SpaceAfterFunction,
    };
    use luck_linter::RuleId;

    use super::*;
    use crate::fixtures::parsed;
    use crate::parse::parse;

    /// The message `json` fails to parse with.
    fn parse_error(json: &str) -> String {
        parse(json).expect_err("config doesn't parse").message.to_string()
    }

    const COMMITTED_SCHEMA_PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../editors/vscode/schemas/luckrc.schema.json");

    /// Shared by the regenerator and the drift check so they can't disagree.
    fn luckrc_schema_json() -> String {
        let schema = schemars::schema_for!(LuckConfig);
        let mut pretty = serde_json::to_string_pretty(&schema).expect("schema serializes");
        pretty.push('\n');
        pretty
    }

    #[test]
    #[ignore = "writes the committed schema; run with --ignored to refresh"]
    fn regenerate_luckrc_schema() {
        fs::write(COMMITTED_SCHEMA_PATH, luckrc_schema_json()).expect("write schema");
    }

    #[test]
    fn luckrc_schema_is_up_to_date() {
        let committed = fs::read_to_string(COMMITTED_SCHEMA_PATH).expect("read schema");
        assert_eq!(
            committed,
            luckrc_schema_json(),
            "luckrc.schema.json is stale; regenerate it with `just schema`"
        );
    }

    #[test]
    fn schema_takes_the_perf_category_alias() {
        let schema = serde_json::to_value(schemars::schema_for!(LuckConfig)).expect("schema");
        let categories = &schema["$defs"]["CategoryLevels"]["properties"];
        assert_eq!(categories["perf"], categories["performance"]);
    }

    #[test]
    fn schema_names_every_variant_the_config_takes() {
        let schema = serde_json::to_value(schemars::schema_for!(LuckConfig)).expect("schema");
        let variants = serde_json::to_string(&schema["$defs"]["Variant"]).expect("JSON");
        for name in ["standalone", "roblox"] {
            assert!(variants.contains(&format!("\"{name}\"")), "{name}: {variants}");
        }
    }

    #[test]
    fn parses_variant_and_build_keys() {
        let config = parsed(
            r#"{
            "variant": "roblox",
            "entry": "src/main.luau",
            "output_dir": "dist"
        }"#,
        );
        assert_eq!(config.variant, Some(Variant::Roblox));
        assert_eq!(config.entry.as_deref(), Some("src/main.luau"));
        assert_eq!(config.output_dir.as_deref(), Some("dist"));
    }

    #[test]
    fn parses_entries_and_profiles() {
        let config = parsed(
            r#"{
            "entries": [
                {"entry": "src/main.luau", "output": "dist/main.luau"},
                {"entry": "src/other.luau", "output": "dist/other.luau"}
            ],
            "profiles": { "release": { "minify": true } }
        }"#,
        );
        let Entries(entries) = config.entries.expect("entries present");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry, "src/main.luau");
        assert_eq!(entries[0].output, "dist/main.luau");
        let profiles = config.profiles.expect("profiles present");
        assert_eq!(profiles.get("release").and_then(|release| release.minify), Some(true));
    }

    #[test]
    fn parses_profile_transforms_apart_from_top_level_ones() {
        let config = parsed(
            r#"{
            "minify": false,
            "transforms": { "fold_constants": true, "rename_locals": false },
            "profiles": {
                "release": {
                    "minify": true,
                    "transforms": { "fold_constants": false, "rename_locals": true }
                }
            }
        }"#,
        );
        let base_transforms = config.transforms.expect("test config has transforms");
        assert!(base_transforms.fold_constants);
        assert!(!base_transforms.rename_locals);
        let profiles = config.profiles.expect("test config has profiles");
        let release = profiles.get("release").expect("test config has release profile");
        let release_transforms = release.transforms.expect("release profile has transforms");
        assert!(!release_transforms.fold_constants);
        assert!(release_transforms.rename_locals);
    }

    #[test]
    fn omitted_transform_keys_keep_their_defaults() {
        let config = parsed(r#"{ "transforms": { "fold_constants": false } }"#);
        let transforms = config.transforms.expect("transforms present");
        assert!(!transforms.fold_constants);
        assert!(transforms.rename_locals);
        assert!(!transforms.rename_globals);
    }

    #[test]
    fn parses_overrides() {
        let config = parsed(
            r#"{ "overrides": [{
                "files": ["spec/**"],
                "exclude_files": ["spec/fixtures/**"],
                "format": { "line_width": 80 },
                "lint": { "rules": { "shadowing": "off" } }
            }] }"#,
        );
        let overrides = config.overrides.expect("overrides present");
        let [spec] = overrides.as_slice() else { panic!("one override: {overrides:?}") };
        assert_eq!(spec.files.iter().map(Glob::pattern).collect::<Vec<_>>(), ["spec/**"]);
        assert_eq!(spec.exclude_files.len(), 1);
        assert_eq!(spec.format.and_then(|format| format.line_width), Some(80));
        assert!(spec.lint.is_some());
    }

    #[test]
    fn merge_onto_layers_lists_and_keeps_unset_base_keys() {
        let base = parsed(
            r#"{ "variant": "roblox", "include": ["a/**", "b/**"],
            "lint": { "extra_globals": ["a"], "rules": { "shadowing": "off" } } }"#,
        );
        let child = parsed(
            r#"{ "include": ["c/**", "a/**"],
            "lint": { "extra_globals": ["b"], "rules": { "empty_block": "error" } } }"#,
        );
        let merged = child.merge_onto(base);
        assert_eq!(merged.variant, Some(Variant::Roblox));
        let include: Vec<&str> = merged.include.iter().flatten().map(Glob::pattern).collect();
        assert_eq!(include, ["b/**", "c/**", "a/**"]);
        let lint = merged.lint.unwrap();
        assert_eq!(lint.extra_globals, ["a", "b"]);
        let rules: Vec<&str> = lint.rules.rules().map(RuleId::name).collect();
        assert_eq!(rules, ["empty_block", "shadowing"]);
    }

    #[test]
    fn merge_onto_lists_the_base_overrides_first() {
        let base = parsed(r#"{ "overrides": [{ "files": ["a/**"] }, { "files": ["b/**"] }] }"#);
        let child = parsed(r#"{ "overrides": [{ "files": ["c/**"] }, { "files": ["a/**"] }] }"#);
        let overrides = child.merge_onto(base).overrides.unwrap();
        let files: Vec<&str> = overrides.iter().map(|layer| layer.files[0].pattern()).collect();
        assert_eq!(files, ["b/**", "c/**", "a/**"]);
    }

    #[test]
    fn parses_every_format_option() {
        let json = r#"{
            "format": {
                "line_width": 120,
                "indent_style": "spaces",
                "indent_width": 4,
                "quote_style": "double",
                "hexadecimal_case": "upper",
                "call_parentheses": "input",
                "collapse_simple_statement": "conditional_only",
                "line_endings": "windows",
                "block_newline_gaps": "preserve",
                "sort_requires": true,
                "space_after_function_names": "calls",
                "magic_trailing_comma": true
            }
        }"#;
        let format = parsed(json).format.expect("format should be present");
        assert_eq!(format.line_width, Some(120));
        assert_eq!(format.indent_style, Some(IndentStyle::Spaces));
        assert_eq!(format.indent_width, Some(4));
        assert_eq!(format.quote_style, Some(QuoteStyle::Double));
        assert_eq!(format.hexadecimal_case, Some(HexCase::Upper));
        assert_eq!(format.call_parentheses, Some(CallParentheses::Input));
        assert_eq!(
            format.collapse_simple_statement,
            Some(CollapseSimpleStatement::ConditionalOnly)
        );
        assert_eq!(format.line_endings, Some(LineEndings::Windows));
        assert_eq!(format.block_newline_gaps, Some(BlockNewlineGaps::Preserve));
        assert_eq!(format.sort_requires, Some(true));
        assert_eq!(format.space_after_function_names, Some(SpaceAfterFunction::Calls));
        assert_eq!(format.magic_trailing_comma, Some(true));
    }

    #[test]
    fn leaves_unset_format_options_none() {
        let config = parsed(r#"{ "format": {} }"#);
        let FormatConfig {
            line_width,
            indent_style,
            indent_width,
            quote_style,
            hexadecimal_case,
            call_parentheses,
            collapse_simple_statement,
            line_endings,
            block_newline_gaps,
            sort_requires,
            space_after_function_names,
            magic_trailing_comma,
        } = config.format.expect("format should be present");
        assert!(line_width.is_none());
        assert!(indent_style.is_none());
        assert!(indent_width.is_none());
        assert!(quote_style.is_none());
        assert!(hexadecimal_case.is_none());
        assert!(call_parentheses.is_none());
        assert!(collapse_simple_statement.is_none());
        assert!(line_endings.is_none());
        assert!(block_newline_gaps.is_none());
        assert!(sort_requires.is_none());
        assert!(space_after_function_names.is_none());
        assert!(magic_trailing_comma.is_none());
    }

    #[test]
    fn rejects_an_unknown_format_key() {
        let error =
            parse_error(r#"{ "format": { "line_width": 100, "not_a_real_option": true } }"#);
        assert!(error.contains("unknown field `not_a_real_option`"), "{error}");
    }

    #[test]
    fn rejects_an_unknown_quote_style() {
        let error = parse_error(r#"{ "format": { "quote_style": "bogus" } }"#);
        assert!(error.contains("unknown variant `bogus`"), "{error}");
    }

    #[test]
    fn rejects_the_function_collapse_modes() {
        for mode in ["function_only", "always"] {
            let json = format!(r#"{{ "format": {{ "collapse_simple_statement": "{mode}" }} }}"#);
            let error = parse_error(&json);
            assert!(error.contains(mode), "error should name the offending value: {error}");
        }
    }
}
