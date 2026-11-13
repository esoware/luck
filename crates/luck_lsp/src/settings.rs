//! Discovery walks up from the document's directory. Results, misses
//! included, stay cached until a watched config file changes.

use alloc::sync::Arc;
use std::path::{Path, PathBuf};

use luck_config::{
    DialectKey, FormatConfig, LuckConfig, ProjectFilter, discover_config, resolved_format_config,
};
use luck_core::LuaTarget;
use luck_formatter::FormatOptions;
use luck_linter::{Category, LintConfig, RuleLevel};
use rustc_hash::FxHashMap;
use tokio::sync::RwLock;
use tokio::task::spawn_blocking;

#[derive(Debug)]
pub(crate) struct ProjectSettings {
    pub lua_target: LuaTarget,
    pub luau_target: LuaTarget,
    /// The raw `format` section, kept to layer over each file's
    /// `.editorconfig` at format time.
    pub format_config: Option<FormatConfig>,
    /// The lint config to run. Linting is opt-in, so without a `lint` section
    /// `correctness` is off and only parse errors surface.
    ///
    /// Publishing, code actions, and fix-all all go through this, so none of
    /// them fixes a diagnostic the user was never shown.
    pub lint_config: LintConfig,
    pub filter: ProjectFilter,
    /// Lua 5.x template search paths (`?.lua`, `src/?.lua`, ...). Resolved
    /// against [`root`](Self::root) when it's set, otherwise against the
    /// requiring file's own directory. Luau targets ignore them and resolve
    /// relative and alias imports instead.
    pub search_paths: Vec<String>,
    /// Directory containing the discovered `luck.json`. `None` means no
    /// config was found or it failed to load, so these are the defaults.
    pub root: Option<PathBuf>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self::new(&LuckConfig::default(), None)
    }
}

impl ProjectSettings {
    /// Settings from `config`, the `luck.json` found in `root`. Without a
    /// root, the filter is rooted at the working directory.
    fn new(config: &LuckConfig, root: Option<PathBuf>) -> Self {
        // Unlike the CLI, the server can't just exit on a config mistake, so a
        // bad dialect or glob falls back to its default.
        let target = |k: DialectKey| config.target(k).unwrap_or_else(|_| k.default_target());
        let filter_base = root.as_deref().unwrap_or_else(|| Path::new("."));
        Self {
            lua_target: target(DialectKey::Lua),
            luau_target: target(DialectKey::Luau),
            format_config: config.format,
            lint_config: config.lint.clone().unwrap_or_else(|| {
                let mut lint_config = LintConfig::default();
                lint_config.categories.set_level(Category::Correctness, RuleLevel::Off);
                lint_config
            }),
            // Without `include`/`exclude` the default globs are constants that
            // always build, so the fallback's `unwrap_or_default` is never
            // reached.
            filter: config.project_filter(filter_base).unwrap_or_else(|_| {
                ProjectFilter::new(filter_base, None, None).unwrap_or_default()
            }),
            search_paths: config.search_paths_or_default(),
            root,
        }
    }

    /// The target for the document at `file_path`, picked by the
    /// [`DialectKey`] its extension selects. A document with no file path
    /// takes the `lua` key's.
    pub(crate) fn target_for(&self, file_path: Option<&Path>) -> LuaTarget {
        match file_path.map_or(DialectKey::Lua, DialectKey::for_path) {
            DialectKey::Lua => self.lua_target,
            DialectKey::Luau => self.luau_target,
        }
    }

    /// Precedence is defaults < `.editorconfig` < `luck.json` `format`.
    /// Untitled and other non-file documents have no `.editorconfig` to find,
    /// so they get the project's `format` section alone.
    pub(crate) fn format_options(&self, file_path: Option<&Path>) -> FormatOptions {
        FormatOptions::from(&resolved_format_config(self.format_config, file_path))
    }
}

#[derive(Debug, Default)]
pub(crate) struct SettingsCache {
    /// Shared by every document without a config or a file path.
    defaults: Arc<ProjectSettings>,
    /// One lock for both maps, so a reader never sees a root without its
    /// settings.
    lookups: RwLock<Lookups>,
}

#[derive(Debug, Default)]
struct Lookups {
    settings_by_root: FxHashMap<PathBuf, Arc<ProjectSettings>>,
    /// Each search directory's config root, or `None` for no config, so a
    /// repeat lookup doesn't walk the filesystem again.
    roots_by_search_directory: FxHashMap<PathBuf, Option<PathBuf>>,
}

impl SettingsCache {
    /// Resolve settings for the document at `document_path`. Walks parent
    /// directories looking for `luck.json`, caches the result, and falls back
    /// to defaults if discovery fails, no config exists, or the document has
    /// no file path.
    pub(crate) async fn settings_for(&self, document_path: Option<&Path>) -> Arc<ProjectSettings> {
        let Some(file_path) = document_path else {
            return Arc::clone(&self.defaults);
        };
        let search_directory =
            file_path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);

        {
            let lookups = self.lookups.read().await;
            match lookups.roots_by_search_directory.get(&search_directory) {
                Some(Some(root)) => {
                    if let Some(settings) = lookups.settings_by_root.get(root) {
                        return Arc::clone(settings);
                    }
                }
                Some(None) => return Arc::clone(&self.defaults),
                None => {}
            }
        }

        // Discovery reads config files and canonicalizes the root.
        let discovery_directory = search_directory.clone();
        let discovered = spawn_blocking(move || {
            let project = discover_config(&discovery_directory).ok().flatten()?;
            let settings = ProjectSettings::new(&project.config, Some(project.root.clone()));
            Some((project.root, Arc::new(settings)))
        })
        .await
        .ok()
        .flatten();
        let Some((root, settings)) = discovered else {
            // Cache the negative lookup so we don't re-walk every did_change.
            self.lookups.write().await.roots_by_search_directory.insert(search_directory, None);
            return Arc::clone(&self.defaults);
        };
        let mut lookups = self.lookups.write().await;
        lookups.settings_by_root.insert(root.clone(), Arc::clone(&settings));
        lookups.roots_by_search_directory.insert(search_directory, Some(root));
        settings
    }

    pub(crate) async fn clear(&self) {
        let mut lookups = self.lookups.write().await;
        lookups.settings_by_root.clear();
        lookups.roots_by_search_directory.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_config::parse_luck_config;
    use luck_formatter::{
        BlockNewlineGaps, CallParentheses, IndentStyle, QuoteStyle, SpaceAfterFunction,
    };

    use super::*;

    #[test]
    fn default_settings_are_lua54() {
        let settings = ProjectSettings::default();
        assert_eq!(settings.lua_target, LuaTarget::Lua54);
        assert!(settings.root.is_none());
    }

    #[test]
    fn target_for_picks_by_extension() {
        let settings = ProjectSettings {
            lua_target: LuaTarget::Lua53,
            luau_target: LuaTarget::LuauRoblox,
            ..Default::default()
        };
        assert_eq!(settings.target_for(Some(Path::new("/tmp/a.lua"))), LuaTarget::Lua53);
        assert_eq!(settings.target_for(Some(Path::new("/tmp/a.luau"))), LuaTarget::LuauRoblox);
        // Unknown extension falls back to the lua target.
        assert_eq!(settings.target_for(Some(Path::new("/tmp/a.txt"))), LuaTarget::Lua53);
        assert_eq!(settings.target_for(None), LuaTarget::Lua53);
    }

    #[tokio::test]
    async fn cache_falls_back_to_defaults() {
        let cache = SettingsCache::default();
        let settings =
            cache.settings_for(Some(Path::new("/definitely/nonexistent/path/foo.lua"))).await;
        assert_eq!(settings.lua_target, LuaTarget::Lua54);
    }

    #[tokio::test]
    async fn cache_keeps_settings_until_cleared() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, r#"{ "lua": "lua53" }"#).expect("write luck.json");
        let first = directory.path().join("first.lua");
        let second = directory.path().join("second.lua");
        let cache = SettingsCache::default();

        let settings = cache.settings_for(Some(&first)).await;
        assert_eq!(settings.lua_target, LuaTarget::Lua53);
        assert!(Arc::ptr_eq(&settings, &cache.settings_for(Some(&second)).await));

        fs::write(&config_path, r#"{ "lua": "lua51" }"#).expect("rewrite luck.json");
        assert_eq!(cache.settings_for(Some(&first)).await.lua_target, LuaTarget::Lua53);
        cache.clear().await;
        assert_eq!(cache.settings_for(Some(&first)).await.lua_target, LuaTarget::Lua51);
    }

    #[test]
    fn settings_carry_targets_lint_and_filter() {
        let json = r#"{ "lua":"lua53","luau":"roblox","lint":{"extra_globals":["vim"]},
            "exclude":["**/vendor/**"] }"#;
        let config = parse_luck_config(json).unwrap();
        let settings = ProjectSettings::new(&config, Some(PathBuf::from("/proj")));
        assert_eq!(settings.lint_config.categories.level(Category::Correctness), None);
        assert_eq!(settings.lint_config.extra_globals, vec!["vim".to_owned()]);
        assert_eq!(settings.lua_target, LuaTarget::Lua53);
        assert_eq!(settings.luau_target, LuaTarget::LuauRoblox);
        assert!(!settings.filter.is_included(Path::new("/proj/vendor/a.lua")));
    }

    #[test]
    fn lint_disabled_without_lint_key() {
        let config = parse_luck_config(r#"{ "lua":"lua54" }"#).unwrap();
        let lint_config = ProjectSettings::new(&config, Some(PathBuf::from("."))).lint_config;
        assert_eq!(lint_config.categories.level(Category::Correctness), Some(RuleLevel::Off));
        assert_eq!(lint_config.rules.names().count(), 0);
    }

    #[test]
    fn format_options_without_a_path_use_the_format_section() {
        let json = r#"{
            "luau": "luau",
            "entry": "main.luau",
            "output_dir": "dist",
            "format": {
                "line_width": 120,
                "indent_style": "spaces",
                "indent_width": 2,
                "quote_style": "single"
            }
        }"#;
        let config = parse_luck_config(json).expect("parse config");
        let settings = ProjectSettings::new(&config, Some(PathBuf::from(".")));
        assert_eq!(settings.luau_target, LuaTarget::Luau);
        let options = settings.format_options(None);
        assert_eq!(options.line_width, 120);
        assert_eq!(options.indent_width, 2);
        assert_eq!(options.indent_style, IndentStyle::Spaces);
        assert_eq!(options.quote_style, QuoteStyle::Single);
    }

    #[test]
    fn maps_all_formatter_options() {
        let json = r#"{ "format":{ "block_newline_gaps":"preserve","sort_requires":true,
            "space_after_function_names":"calls","magic_trailing_comma":true,"call_parentheses":"input" } }"#;
        let config = parse_luck_config(json).unwrap();
        let options = ProjectSettings::new(&config, Some(PathBuf::from("."))).format_options(None);
        assert_eq!(options.block_newline_gaps, BlockNewlineGaps::Preserve);
        assert!(options.sort_requires);
        assert_eq!(options.space_after_function_names, SpaceAfterFunction::Calls);
        assert!(options.magic_trailing_comma);
        assert_eq!(options.call_parentheses, CallParentheses::Input);
    }

    #[test]
    fn format_options_pick_up_editorconfig() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join(".editorconfig"),
            "[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write editorconfig");
        let file_path = directory.path().join("main.lua");

        // No project format config: the `.editorconfig` is the only source.
        let options = ProjectSettings::default().format_options(Some(&file_path));
        assert_eq!(options.indent_style, IndentStyle::Spaces);
        assert_eq!(options.indent_width, 2);
    }

    #[test]
    fn format_options_luck_json_overrides_editorconfig() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join(".editorconfig"),
            "[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write editorconfig");
        let file_path = directory.path().join("main.lua");

        // luck.json `format` sets indent_width to 8, which must win over the
        // `.editorconfig` value of 2.
        let config = parse_luck_config(r#"{ "format": { "indent_width": 8 } }"#).unwrap();
        let settings = ProjectSettings::new(&config, Some(PathBuf::from(".")));
        let options = settings.format_options(Some(&file_path));
        assert_eq!(options.indent_width, 8);
        // indent_style not set in luck.json, so it still comes from editorconfig.
        assert_eq!(options.indent_style, IndentStyle::Spaces);
    }
}
