//! Nothing here touches the filesystem.

use core::hash::Hash;
use std::path::Path;

use luck_core::LuaTarget;
use luck_linter::LintConfig;
use luck_resolver::DEFAULT_SEARCH_PATHS;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dynamic_require::DynamicRequire;
use crate::filter::ProjectFilter;
use crate::format_config::FormatConfig;
use crate::transform_config::TransformConfig;

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
#[derive(Debug, Clone, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EntryConfig {
    pub entry: String,
    pub output: String,
}

/// A build profile, selected with `luck build --profile <name>`.
///
/// Its `minify` and `transforms` replace the top-level ones. `--release` and
/// `--dev` select the `release` and `dev` profiles.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProfileOverrides {
    pub minify: Option<bool>,
    pub transforms: Option<TransformConfig>,
}

/// Top-level luck.json configuration.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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

impl LuckConfig {
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
            |dialect| dialect.parse::<LuaTarget>().map_err(|e| format!("`{name}`: {e}")),
        )
    }

    /// Only that key is validated.
    ///
    /// # Errors
    ///
    /// Returns a message when the selected key names no known target.
    pub fn target_for_path(&self, path: &Path) -> Result<LuaTarget, String> {
        self.target(DialectKey::for_path(path))
    }

    /// # Errors
    ///
    /// Returns a message naming the first invalid glob pattern.
    pub fn project_filter(&self, root: &Path) -> Result<ProjectFilter, String> {
        ProjectFilter::new(root, self.include.as_deref(), self.exclude.as_deref())
    }

    /// An empty list reaches the resolver, which treats it as the defaults too.
    #[must_use]
    pub fn search_paths_or_default(&self) -> Vec<String> {
        self.search_paths
            .clone()
            .unwrap_or_else(|| DEFAULT_SEARCH_PATHS.iter().map(ToString::to_string).collect())
    }

    /// `include`, `exclude`, and `search_paths` concatenate base-first without
    /// repeats, `format` and `lint` merge field by field, and `extends` is
    /// cleared.
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

/// `base` followed by the items of `child` it lacks, so a file layered twice
/// through `extends` can't double a list.
fn concat<T>(child: Vec<T>, mut base: Vec<T>) -> Vec<T>
where
    T: Eq + Hash,
{
    let present: FxHashSet<&T> = base.iter().collect();
    let added: Vec<T> = child.into_iter().filter(|item| !present.contains(item)).collect();
    base.extend(added);
    base
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::load::parse_luck_config;

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
        // core.autocrlf may have checked the file out with CRLF endings.
        let committed =
            fs::read_to_string(COMMITTED_SCHEMA_PATH).expect("read schema").replace("\r\n", "\n");
        assert_eq!(
            committed,
            luckrc_schema_json(),
            "luckrc.schema.json is stale; regenerate it with `just schema`"
        );
    }

    #[test]
    fn parses_dialect_and_build_keys() {
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
    fn parses_a_minimal_luau_config() {
        let json = r#"{"luau": "luau", "entry": "main.luau", "output_dir": "out"}"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.luau.as_deref(), Some("luau"));
        assert_eq!(config.entry.as_deref(), Some("main.luau"));
        assert!(config.search_paths.is_none());
        assert!(config.minify.is_none());
    }

    #[test]
    fn parses_entries() {
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
    fn parses_profiles() {
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
    fn parses_output() {
        let json = r#"{
            "lua": "lua54",
            "entry": "src/main.lua",
            "output": "dist/bundle.lua"
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        assert_eq!(config.output.as_deref(), Some("dist/bundle.lua"));
    }

    #[test]
    fn leaves_unset_keys_none() {
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
    fn rejects_unknown_top_level_keys() {
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
    fn parses_profile_transforms_apart_from_top_level_ones() {
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

    /// The conflict is reported when the build resolves, not at parse time.
    #[test]
    fn parses_entry_and_entries_together() {
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
    fn parses_preamble_and_luck_preamble() {
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
    fn parses_dialect_and_scope_keys() {
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
    fn target_for_path_picks_the_key_by_extension() {
        let config = parse_luck_config(r#"{ "lua":"lua53","luau":"roblox" }"#).unwrap();
        assert_eq!(config.target_for_path(Path::new("a.lua")).unwrap(), LuaTarget::Lua53);
        assert_eq!(config.target_for_path(Path::new("a.luau")).unwrap(), LuaTarget::LuauRoblox);
    }

    #[test]
    fn target_defaults_an_unset_key_and_names_an_invalid_one() {
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
            "rules":{"x":"off"}} }"#,
        )
        .unwrap();
        let child = parse_luck_config(
            r#"{ "lint":{"extra_globals":["b"],
            "rules":{"y":"error"}} }"#,
        )
        .unwrap();
        let merged = child.merge_onto(base);
        let lint = merged.lint.unwrap();
        assert_eq!(lint.extra_globals, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(lint.rules.names().collect::<Vec<_>>(), ["x", "y"]);
        assert_eq!(merged.lua.as_deref(), Some("lua54"));
    }
}
