//! The deserialized `luck.json` types and how `extends` merges them.
//!
//! Every type here denies unknown fields and derives `schemars::JsonSchema`;
//! the VS Code schema is generated from [`LuckConfig`], and the type names
//! are its `$defs` keys. Nothing here touches the filesystem.

use alloc::collections::BTreeMap;
use std::path::Path;

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::diagnostic::DiagnosticSeverity;
use crate::dynamic_require::DynamicRequire;
use crate::format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, HexCase, IndentStyle, LineEndings,
    QuoteStyle, SpaceAfterFunction,
};
use crate::lua_target::LuaTarget;
use crate::transform_config::TransformConfig;

/// Lua 5.x search path templates used when `search_paths` isn't set.
pub const DEFAULT_SEARCH_PATHS: &[&str] = &["?.lua", "?/init.lua"];

/// One of the two `luck.json` keys that name a dialect by file extension.
///
/// A key names a dialect, not a language family, so `"lua": "roblox"` is how
/// a Roblox or Rojo tree says its `.lua` files are Luau.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialectKey {
    /// `lua`, for every file but a `.luau` one.
    Lua,
    /// `luau`, for `.luau` files.
    Luau,
}

impl DialectKey {
    #[must_use]
    pub fn for_path(path: &Path) -> Self {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("luau") => Self::Luau,
            _ => Self::Lua,
        }
    }

    /// The dialect when `luck.json` leaves the key unset: Lua 5.4 for `lua`,
    /// standalone Luau for `luau`.
    #[must_use]
    pub const fn default_target(self) -> LuaTarget {
        match self {
            Self::Lua => LuaTarget::Lua54,
            Self::Luau => LuaTarget::Luau,
        }
    }
}

/// One entry of a multi-entry build: the file to bundle and where to write
/// the bundle, both relative to the `luck.json` directory.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryConfig {
    pub entry: String,
    pub output: String,
}

/// A build profile, selected with `luck build --profile <name>`.
///
/// Its `minify` and `transforms` replace the top-level ones. `--release` and
/// `--dev` select the `release` and `dev` profiles.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileOverrides {
    pub minify: Option<bool>,
    pub transforms: Option<TransformConfig>,
}

// Lives here so `luck.json` deserializes without depending on the linter.
/// A group of lint rules, enabled together through `categories`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Outright wrong or useless code.
    Correctness,
    /// Most likely wrong or useless.
    Suspicious,
    /// Idiomatic code style.
    Style,
    /// Performance improvements.
    #[serde(alias = "perf")]
    Performance,
}

/// Overrides for one lint rule.
///
/// `enabled` turns the rule on or off, and `severity` replaces its default
/// severity. Either can be set alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleSetting {
    pub enabled: Option<bool>,
    pub severity: Option<DiagnosticSeverity>,
}

/// Linter settings from luck.json.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LintConfig {
    /// Per-rule overrides, keyed by rule name.
    #[serde(default)]
    pub rule_overrides: BTreeMap<String, RuleSetting>,
    /// Extra global names to treat as defined, such as `vim` or `love`.
    #[serde(default)]
    pub extra_globals: Vec<String>,
    /// Modules the `restricted_module_paths` rule forbids requiring. A
    /// trailing `.*` also covers submodules: `forbidden.*` matches `forbidden`
    /// and `forbidden.sub`.
    #[serde(default)]
    pub restricted_module_paths: Vec<String>,
    /// Complexity above which the `cyclomatic_complexity` rule, a `style`
    /// rule, flags a function. The rule reports nothing while this is unset.
    #[serde(default)]
    pub max_cyclomatic_complexity: Option<u32>,
    /// When true, only rules set to `"enabled": true` in `rule_overrides` run.
    /// Correctness rules and `categories` are ignored.
    #[serde(default)]
    pub disable_default_rules: bool,
    /// Rule categories to enable on top of `correctness`, which is on by
    /// default.
    #[serde(default)]
    pub categories: Vec<Category>,
}

/// Formatter settings from luck.json.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormatConfig {
    pub line_width: Option<u16>,
    pub indent_style: Option<IndentStyle>,
    pub indent_width: Option<u8>,
    pub quote_style: Option<QuoteStyle>,
    /// Case of the hex digits `A`-`F` in numeric literals: `"preserve"`
    /// (default), `"lower"`, or `"upper"`. The `0x` prefix is always
    /// lowercase.
    pub hexadecimal_case: Option<HexCase>,
    pub call_parentheses: Option<CallParentheses>,
    pub collapse_simple_statement: Option<CollapseSimpleStatement>,
    pub line_endings: Option<LineEndings>,
    /// Whether to keep a blank line at the start or end of a block body:
    /// `"never"` (default) or `"preserve"`.
    pub block_newline_gaps: Option<BlockNewlineGaps>,
    /// When true, sorts each run of consecutive `local NAME = require(...)`
    /// or `local NAME = game:GetService(...)` statements by name. A blank
    /// line ends a run.
    pub sort_requires: Option<bool>,
    /// Where to put a space before `(`: `"never"` (default), `"definitions"`,
    /// `"calls"`, or `"always"` for both.
    pub space_after_function_names: Option<SpaceAfterFunction>,
    /// When true, a trailing comma in a table constructor breaks the table one
    /// field per line, as in Black and Prettier. Defaults to `false`. Argument
    /// lists can't carry one, since no dialect allows a comma after the last
    /// argument.
    pub magic_trailing_comma: Option<bool>,
}

/// Top-level luck.json configuration.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LuckConfig {
    /// Dialect for `.lua` files (default `lua54`). Any dialect is accepted:
    /// Roblox and Rojo trees that keep Luau in `.lua` files set
    /// `"lua": "roblox"` or `"lua": "luau"`.
    pub lua: Option<String>,
    /// Dialect for `.luau` files (default `luau`). Any dialect is accepted.
    pub luau: Option<String>,
    pub root: Option<bool>,
    pub extends: Option<Vec<String>>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub entry: Option<String>,
    pub output_dir: Option<String>,
    pub output: Option<String>,
    pub entries: Option<Vec<EntryConfig>>,
    pub minify: Option<bool>,
    /// How the bundler treats a `require` whose argument isn't a string
    /// literal: `"error"`, `"warn"` (default), or `"allow"`.
    pub dynamic_require: Option<DynamicRequire>,
    pub search_paths: Option<Vec<String>>,
    pub transforms: Option<TransformConfig>,
    pub profiles: Option<FxHashMap<String, ProfileOverrides>>,
    pub preamble: Option<String>,
    pub luck_preamble: Option<bool>,
    pub format: Option<FormatConfig>,
    pub lint: Option<LintConfig>,
}

impl LintConfig {
    /// Merges `self` over `base`.
    ///
    /// Lists concatenate base-first, a `rule_overrides` entry from `self`
    /// replaces the base's entry for the same rule, and
    /// `max_cyclomatic_complexity` is self-or-base. `disable_default_rules` is
    /// true if either side sets it: a bare `bool` can't tell "omitted" from
    /// `false`, and silently re-enabling a parent's disabled defaults is the
    /// worse failure.
    fn merge_onto(self, mut base: Self) -> Self {
        base.rule_overrides.extend(self.rule_overrides);
        base.extra_globals.extend(self.extra_globals);
        base.restricted_module_paths.extend(self.restricted_module_paths);
        base.categories.extend(self.categories);
        Self {
            rule_overrides: base.rule_overrides,
            extra_globals: base.extra_globals,
            restricted_module_paths: base.restricted_module_paths,
            max_cyclomatic_complexity: self
                .max_cyclomatic_complexity
                .or(base.max_cyclomatic_complexity),
            disable_default_rules: self.disable_default_rules || base.disable_default_rules,
            categories: base.categories,
        }
    }
}

impl FormatConfig {
    #[must_use]
    pub(crate) fn merge_onto(self, base: Self) -> Self {
        Self {
            line_width: self.line_width.or(base.line_width),
            indent_style: self.indent_style.or(base.indent_style),
            indent_width: self.indent_width.or(base.indent_width),
            quote_style: self.quote_style.or(base.quote_style),
            hexadecimal_case: self.hexadecimal_case.or(base.hexadecimal_case),
            call_parentheses: self.call_parentheses.or(base.call_parentheses),
            collapse_simple_statement: self
                .collapse_simple_statement
                .or(base.collapse_simple_statement),
            line_endings: self.line_endings.or(base.line_endings),
            block_newline_gaps: self.block_newline_gaps.or(base.block_newline_gaps),
            sort_requires: self.sort_requires.or(base.sort_requires),
            space_after_function_names: self
                .space_after_function_names
                .or(base.space_after_function_names),
            magic_trailing_comma: self.magic_trailing_comma.or(base.magic_trailing_comma),
        }
    }
}

impl LuckConfig {
    /// Resolves the dialect `key` names, or [`DialectKey::default_target`]
    /// when it's unset.
    ///
    /// # Errors
    ///
    /// Returns a message when `key` names no known target.
    pub fn target(&self, key: DialectKey) -> Result<LuaTarget, String> {
        let (name, setting) = match key {
            DialectKey::Lua => ("lua", &self.lua),
            DialectKey::Luau => ("luau", &self.luau),
        };
        setting.as_ref().map_or_else(
            || Ok(key.default_target()),
            |dialect| dialect.parse::<LuaTarget>().map_err(|error| format!("`{name}`: {error}")),
        )
    }

    /// Resolves the dialect of the file at `path` from the key its extension
    /// selects. Only that key is validated.
    ///
    /// # Errors
    ///
    /// Returns a message when the selected key names no known target.
    pub fn target_for_path(&self, path: &Path) -> Result<LuaTarget, String> {
        self.target(DialectKey::for_path(path))
    }

    /// The Lua search path templates: `search_paths` when it's set, even to an
    /// empty list, and [`DEFAULT_SEARCH_PATHS`] otherwise.
    #[must_use]
    pub fn search_paths_or_default(&self) -> Vec<String> {
        self.search_paths
            .clone()
            .unwrap_or_else(|| DEFAULT_SEARCH_PATHS.iter().map(ToString::to_string).collect())
    }

    /// Merges `self` over `base`, with `self` winning.
    ///
    /// `include`, `exclude`, and `search_paths` concatenate base-first,
    /// `format` and `lint` merge field by field, and `extends` is cleared.
    /// Every other key, including `entries`, `profiles`, and `transforms`,
    /// comes whole from `self` when set there and from `base` otherwise.
    #[must_use]
    pub(crate) fn merge_onto(self, base: Self) -> Self {
        Self {
            lua: self.lua.or(base.lua),
            luau: self.luau.or(base.luau),
            root: self.root.or(base.root),
            extends: None, // already resolved; don't re-expand
            include: merge_with(self.include, base.include, concat),
            exclude: merge_with(self.exclude, base.exclude, concat),
            entry: self.entry.or(base.entry),
            output_dir: self.output_dir.or(base.output_dir),
            output: self.output.or(base.output),
            entries: self.entries.or(base.entries),
            minify: self.minify.or(base.minify),
            dynamic_require: self.dynamic_require.or(base.dynamic_require),
            search_paths: merge_with(self.search_paths, base.search_paths, concat),
            transforms: self.transforms.or(base.transforms),
            profiles: self.profiles.or(base.profiles),
            preamble: self.preamble.or(base.preamble),
            luck_preamble: self.luck_preamble.or(base.luck_preamble),
            format: merge_with(self.format, base.format, FormatConfig::merge_onto),
            lint: merge_with(self.lint, base.lint, LintConfig::merge_onto),
        }
    }
}

fn merge_with<T, Merge>(child: Option<T>, base: Option<T>, merge: Merge) -> Option<T>
where
    Merge: FnOnce(T, T) -> T,
{
    match (child, base) {
        (Some(child_value), Some(base_value)) => Some(merge(child_value, base_value)),
        (child_value, base_value) => child_value.or(base_value),
    }
}

fn concat<T>(child: Vec<T>, mut base: Vec<T>) -> Vec<T> {
    base.extend(child);
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_luck_config;

    #[test]
    fn parse_luck_config_full() {
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
            Some(&["src/?.lua".to_owned(), "lib/?.lua".to_owned()][..])
        );
    }

    #[test]
    fn parse_luck_config_minimal() {
        let json = r#"{"luau": "luau", "entry": "main.luau", "output_dir": "out"}"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert_eq!(config.entry.as_deref(), Some("main.luau"));
        assert!(config.search_paths.is_none());
        assert!(config.minify.is_none());
    }

    #[test]
    fn parse_entries_config() {
        let json = r#"{
            "lua": "lua54",
            "entries": [
                {"entry": "src/main.lua", "output": "dist/main.lua"},
                {"entry": "src/other.lua", "output": "dist/other.lua"}
            ]
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        let entries = config.entries.expect("entries present");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry, "src/main.lua");
        assert_eq!(entries[0].output, "dist/main.lua");
    }

    #[test]
    fn parse_profiles_config() {
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
        let profiles = config.profiles.expect("profiles present");
        let release = profiles.get("release").expect("release profile present");
        assert_eq!(release.minify, Some(true));
    }

    #[test]
    fn parse_output_field() {
        let json = r#"{
            "lua": "lua54",
            "entry": "src/main.lua",
            "output": "dist/bundle.lua"
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.output.as_deref(), Some("dist/bundle.lua"));
    }

    #[test]
    fn missing_optional_fields_default_to_none() {
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
    fn parse_lint_config() {
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
        assert_eq!(lint.extra_globals, vec!["vim".to_owned(), "roblox".to_owned()]);
        assert_eq!(lint.restricted_module_paths, vec!["secret.path".to_owned()]);
        assert_eq!(lint.max_cyclomatic_complexity, Some(15));
        assert!(lint.disable_default_rules);
        let unused_variable =
            lint.rule_overrides.get("unused_variable").expect("unused_variable override");
        assert_eq!(unused_variable.enabled, Some(false));
        assert!(unused_variable.severity.is_none());
        let undefined_variable =
            lint.rule_overrides.get("undefined_variable").expect("undefined_variable override");
        assert_eq!(undefined_variable.severity, Some(DiagnosticSeverity::Error));
    }

    #[test]
    fn parse_lint_config_defaults_when_omitted() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "lint": {}
        }"#;
        let config = parse_luck_config(json).expect("deserialize");
        let lint = config.lint.expect("lint present");
        assert!(lint.rule_overrides.is_empty());
        assert_eq!(lint.extra_globals, Vec::<String>::new());
        assert_eq!(lint.restricted_module_paths, Vec::<String>::new());
        assert!(lint.max_cyclomatic_complexity.is_none());
        assert!(!lint.disable_default_rules);
    }

    #[test]
    fn unknown_keys_rejected() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "unknown_key": true,
            "also_unknown": 42
        }"#;
        let result = parse_luck_config(json);
        assert!(result.is_err(), "unknown top-level keys must error");
        let error = result.unwrap_err();
        assert!(
            error.contains("unknown_key") || error.contains("unknown field"),
            "error should name the offending key: {error}"
        );
    }

    #[test]
    fn unknown_nested_key_rejected() {
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
        let error = result.unwrap_err();
        assert!(
            error.contains("not_a_real_option") || error.contains("unknown field"),
            "error should name the offending nested key: {error}"
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
        let error = result.unwrap_err();
        assert!(
            error.contains("bogus") || error.contains("unknown variant"),
            "error should name the offending value: {error}"
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
        let error = result.unwrap_err();
        assert!(
            error.contains("fatal") || error.contains("unknown variant"),
            "error should name the offending value: {error}"
        );
    }

    #[test]
    fn profile_override_with_transforms() {
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
        let base_transforms = config.transforms.as_ref().expect("test config has transforms");
        assert!(base_transforms.fold_constants);
        assert!(!base_transforms.rename_locals);

        let profiles = config.profiles.as_ref().expect("test config has profiles");
        let release = profiles.get("release").expect("test config has release profile");
        assert_eq!(release.minify, Some(true));
        let release_transforms =
            release.transforms.as_ref().expect("release profile has transforms");
        assert!(!release_transforms.fold_constants);
        assert!(release_transforms.rename_locals);
    }

    #[test]
    fn parse_format_config() {
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

    /// The conflict is reported when the build resolves, not at parse time.
    #[test]
    fn entry_and_entries_together_still_parse() {
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
    fn preamble_and_luck_preamble() {
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
    fn parse_format_polish_options() {
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
        assert_eq!(format.space_after_function_names, Some(SpaceAfterFunction::Calls));
        assert_eq!(format.magic_trailing_comma, Some(true));
        assert_eq!(format.call_parentheses, Some(CallParentheses::Input));
    }

    #[test]
    fn format_polish_defaults_when_omitted() {
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
        assert_eq!(config.extends, Some(vec!["base.json".to_owned()]));
        assert_eq!(config.include.as_deref().unwrap().len(), 1);
        assert_eq!(config.exclude.as_deref().unwrap().len(), 1);
    }

    #[test]
    fn category_perf_alias_parses() {
        let config = parse_luck_config(r#"{ "lint": { "categories": ["perf"] } }"#).unwrap();
        assert_eq!(config.lint.unwrap().categories, vec![Category::Performance]);
    }

    #[test]
    fn target_resolution_by_extension() {
        let config = parse_luck_config(r#"{ "lua":"lua53","luau":"roblox" }"#).unwrap();
        assert_eq!(config.target_for_path(Path::new("a.lua")).unwrap(), LuaTarget::Lua53);
        assert_eq!(config.target_for_path(Path::new("a.luau")).unwrap(), LuaTarget::LuauRoblox);
    }

    #[test]
    fn target_defaults_and_validation() {
        let config = parse_luck_config("{}").unwrap();
        assert_eq!(config.target_for_path(Path::new("a.lua")).unwrap(), LuaTarget::Lua54);
        assert_eq!(config.target_for_path(Path::new("a.luau")).unwrap(), LuaTarget::Luau);
        let unknown_version =
            parse_luck_config(r#"{ "lua":"lua99" }"#).unwrap().target(DialectKey::Lua).unwrap_err();
        assert!(unknown_version.starts_with("`lua`:"), "{unknown_version}");
        let nonsense_dialect = parse_luck_config(r#"{ "luau":"nonsense" }"#)
            .unwrap()
            .target(DialectKey::Luau)
            .unwrap_err();
        assert!(nonsense_dialect.starts_with("`luau`:"), "{nonsense_dialect}");
    }

    /// Extension and dialect are independent, so a Roblox or Rojo tree that
    /// keeps Luau in `.lua` files can say so.
    #[test]
    fn lua_extension_can_name_a_luau_dialect() {
        let roblox_lua = parse_luck_config(r#"{ "lua":"roblox" }"#).unwrap();
        assert_eq!(
            roblox_lua.target_for_path(Path::new("src/init.lua")).unwrap(),
            LuaTarget::LuauRoblox
        );
        // `.luau` files keep their own default and don't follow `lua`.
        assert_eq!(
            roblox_lua.target_for_path(Path::new("src/init.luau")).unwrap(),
            LuaTarget::Luau
        );

        let swapped = parse_luck_config(r#"{ "lua":"luau","luau":"lua51" }"#).unwrap();
        assert_eq!(swapped.target_for_path(Path::new("a.lua")).unwrap(), LuaTarget::Luau);
        assert_eq!(swapped.target_for_path(Path::new("a.luau")).unwrap(), LuaTarget::Lua51);
    }

    #[test]
    fn merge_onto_concatenates_lint_lists_and_keeps_unset_base_keys() {
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
        assert_eq!(lint.extra_globals, vec!["a".to_owned(), "b".to_owned()]);
        assert!(lint.rule_overrides.contains_key("x") && lint.rule_overrides.contains_key("y"));
        assert_eq!(merged.lua.as_deref(), Some("lua54"));
    }

    #[test]
    fn merge_lint_categories_complexity_and_lockdown() {
        // The child omits `disable_default_rules`, so the base's lockdown
        // must survive.
        let base = parse_luck_config(
            r#"{ "lua":"lua54","lint":{"categories":["style"],
            "max_cyclomatic_complexity":10,"disable_default_rules":true}}"#,
        )
        .unwrap();
        let child = parse_luck_config(r#"{ "lint":{"categories":["performance"]} }"#).unwrap();
        let lint = child.merge_onto(base).lint.unwrap();
        assert_eq!(lint.categories, vec![Category::Style, Category::Performance]);
        assert_eq!(lint.max_cyclomatic_complexity, Some(10));
        assert!(lint.disable_default_rules);
    }
}
