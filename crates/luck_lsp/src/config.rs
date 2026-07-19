//! Discover and cache `luck.json` configuration per workspace folder.
//!
//! The LSP walks up the filesystem from each opened document looking for a
//! `luck.json`. The discovered config is cached by the directory that contains
//! it so repeated lookups in the same project are O(1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use luck_core::LuaTarget;
use luck_core::config::{FormatConfig, LuckConfig, discover_config};
use luck_core::editorconfig::resolved_format_config;
use luck_formatter::FormatOptions;
use luck_linter::LintConfig;

/// Resolved per-project settings - what the LSP actually needs at runtime.
#[derive(Debug, Clone)]
pub struct ProjectSettings {
    pub lua_target: LuaTarget,
    pub luau_target: LuaTarget,
    pub format_options: FormatOptions,
    /// Raw project `format` config, kept so per-file `.editorconfig` overlay
    /// can be computed at format time (the file path is needed to walk for it).
    pub format_config: Option<FormatConfig>,
    pub lint_config: LintConfig,
    pub lint_enabled: bool,
    pub filter: luck_core::config::ProjectFilter,
    /// Directory containing the discovered `luck.json`. `None` means we fell
    /// back to defaults because no config was found.
    pub root: Option<PathBuf>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            lua_target: LuaTarget::Lua54,
            luau_target: LuaTarget::Luau,
            format_options: FormatOptions::default(),
            format_config: None,
            lint_config: LintConfig::default(),
            lint_enabled: false,
            filter: luck_core::config::ProjectFilter::new(std::path::Path::new("."), &None, &None)
                .expect("default globs are valid"),
            root: None,
        }
    }
}

impl ProjectSettings {
    /// The lint config to actually run with. Linting is opt-in: a project with
    /// no `lint` section disables all default rules so only parse errors
    /// surface. Every lint pass - published diagnostics, code actions, and
    /// fix-all - MUST go through this so they agree on what is enabled;
    /// otherwise code actions would offer/apply fixes for diagnostics the user
    /// was never shown.
    #[must_use]
    pub fn effective_lint_config(&self) -> LintConfig {
        if self.lint_enabled {
            self.lint_config.clone()
        } else {
            LintConfig {
                disable_default_rules: true,
                ..Default::default()
            }
        }
    }
}

/// Thread-safe cache of project settings keyed by config-file directory.
///
/// Misses fall through to disk discovery; hits return a clone of the cached
/// [`ProjectSettings`].
#[derive(Debug, Default)]
pub struct ConfigCache {
    by_root: RwLock<HashMap<PathBuf, ProjectSettings>>,
    /// Maps a starting search directory to the resolved root, so re-opening a
    /// nested file does not re-walk the filesystem.
    by_search_dir: RwLock<HashMap<PathBuf, Option<PathBuf>>>,
}

impl ConfigCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve settings for the document at `document_path`. Walks parent
    /// directories looking for `luck.json`, caches the result, and falls back
    /// to defaults if discovery fails or no config exists.
    pub fn settings_for(&self, document_path: &Path) -> ProjectSettings {
        let search_dir = document_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        if let Some(cached) = self.lookup_by_search_dir(&search_dir) {
            return cached;
        }

        let discovered = discover_config(&search_dir).ok().flatten();
        match discovered {
            Some((config_path, config)) => {
                let root = config_path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| search_dir.clone());
                let settings = build_settings(&config, Some(root.clone()));
                self.insert(root.clone(), search_dir, Some(root), settings.clone());
                settings
            }
            None => {
                let settings = ProjectSettings::default();
                // Cache the negative lookup so we don't re-walk every did_change.
                self.remember_negative(search_dir);
                settings
            }
        }
    }

    fn lookup_by_search_dir(&self, search_dir: &Path) -> Option<ProjectSettings> {
        let lookup = self.by_search_dir.read().ok()?;
        let root = lookup.get(search_dir)?.clone();
        match root {
            Some(root) => {
                let by_root = self.by_root.read().ok()?;
                by_root.get(&root).cloned()
            }
            None => Some(ProjectSettings::default()),
        }
    }

    fn insert(
        &self,
        root: PathBuf,
        search_dir: PathBuf,
        resolved: Option<PathBuf>,
        settings: ProjectSettings,
    ) {
        if let Ok(mut by_root) = self.by_root.write() {
            by_root.insert(root, settings);
        }
        if let Ok(mut by_search_dir) = self.by_search_dir.write() {
            by_search_dir.insert(search_dir, resolved);
        }
    }

    fn remember_negative(&self, search_dir: PathBuf) {
        if let Ok(mut by_search_dir) = self.by_search_dir.write() {
            by_search_dir.insert(search_dir, None);
        }
    }

    /// Drop every cached entry - useful when `luck.json` changes on disk.
    pub fn clear(&self) {
        if let Ok(mut by_root) = self.by_root.write() {
            by_root.clear();
        }
        if let Ok(mut by_search_dir) = self.by_search_dir.write() {
            by_search_dir.clear();
        }
    }
}

fn build_settings(config: &LuckConfig, root: Option<PathBuf>) -> ProjectSettings {
    let base = root.clone().unwrap_or_else(|| PathBuf::from("."));
    ProjectSettings {
        lua_target: config.lua_target().unwrap_or(LuaTarget::Lua54),
        luau_target: config.luau_target().unwrap_or(LuaTarget::Luau),
        format_options: format_options_from_config(config),
        format_config: config.format.clone(),
        lint_config: config.lint.clone().unwrap_or_default(),
        lint_enabled: config.lint.is_some(),
        // A server must not die on a typo'd glob - fall back to the
        // default filter (the CLI, by contrast, hard-errors).
        filter: luck_core::config::ProjectFilter::new(&base, &config.include, &config.exclude)
            .unwrap_or_else(|_| {
                luck_core::config::ProjectFilter::new(&base, &None, &None)
                    .expect("default globs are valid")
            }),
        root,
    }
}

fn format_options_from_config(config: &LuckConfig) -> FormatOptions {
    match config.format.as_ref() {
        Some(fmt) => FormatOptions::from(fmt),
        None => FormatOptions::default(),
    }
}

/// Resolve the format options to use for a document, applying the per-file
/// `.editorconfig` overlay when a real file path is available.
///
/// Precedence is defaults < `.editorconfig` < `luck.json` `format`. For
/// untitled / non-file documents there is nothing to walk, so we fall back to
/// the precomputed project options.
#[must_use]
pub fn resolved_format_options(
    settings: &ProjectSettings,
    file_path: Option<&Path>,
) -> FormatOptions {
    match file_path {
        Some(path) => {
            let resolved = resolved_format_config(settings.format_config.as_ref(), path, true);
            FormatOptions::from(&resolved)
        }
        None => settings.format_options.clone(),
    }
}

/// Pick the target for `path` from the project's per-extension settings:
/// `.luau` files use `luau_target`, everything else uses `lua_target`.
#[must_use]
pub fn target_from_settings(path: &Path, settings: &ProjectSettings) -> LuaTarget {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("luau") => settings.luau_target,
        _ => settings.lua_target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_formatter::{
        BlockNewlineGaps, CallParentheses, IndentStyle, QuoteStyle, SpaceAfterFunction,
    };

    #[test]
    fn default_settings_are_lua54() {
        let settings = ProjectSettings::default();
        assert_eq!(settings.lua_target, LuaTarget::Lua54);
        assert!(settings.root.is_none());
    }

    #[test]
    fn target_from_settings_per_extension() {
        let settings = ProjectSettings {
            lua_target: LuaTarget::Lua53,
            luau_target: LuaTarget::LuauRoblox,
            ..Default::default()
        };
        assert_eq!(
            target_from_settings(Path::new("/tmp/a.lua"), &settings),
            LuaTarget::Lua53
        );
        assert_eq!(
            target_from_settings(Path::new("/tmp/a.luau"), &settings),
            LuaTarget::LuauRoblox
        );
        // Unknown extension falls back to the lua target.
        assert_eq!(
            target_from_settings(Path::new("/tmp/a.txt"), &settings),
            LuaTarget::Lua53
        );
    }

    #[test]
    fn cache_falls_back_to_defaults() {
        let cache = ConfigCache::new();
        let settings = cache.settings_for(Path::new("/definitely/nonexistent/path/foo.lua"));
        assert_eq!(settings.lua_target, LuaTarget::Lua54);
    }

    #[test]
    fn build_settings_targets_lint_and_filter() {
        let json = r#"{ "lua":"lua53","luau":"roblox","lint":{"extra_globals":["vim"]},
            "exclude":["**/vendor/**"] }"#;
        let config = luck_core::config::parse_luck_config(json).unwrap();
        let s = build_settings(&config, Some(std::path::PathBuf::from("/proj")));
        assert!(s.lint_enabled);
        assert_eq!(s.lint_config.extra_globals, vec!["vim".to_string()]);
        assert_eq!(s.lua_target, LuaTarget::Lua53);
        assert_eq!(s.luau_target, LuaTarget::LuauRoblox);
        assert!(
            !s.filter
                .is_included(std::path::Path::new("/proj/vendor/a.lua"))
        );
    }

    #[test]
    fn lint_disabled_without_lint_key() {
        let config = luck_core::config::parse_luck_config(r#"{ "lua":"lua54" }"#).unwrap();
        assert!(!build_settings(&config, None).lint_enabled);
    }

    #[test]
    fn format_options_round_trip_from_config() {
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
        let config = luck_core::config::parse_luck_config(json).expect("parse config");
        let settings = build_settings(&config, None);
        assert_eq!(settings.luau_target, LuaTarget::Luau);
        assert_eq!(settings.format_options.line_width, 120);
        assert_eq!(settings.format_options.indent_width, 2);
        assert_eq!(settings.format_options.indent_style, IndentStyle::Spaces);
        assert_eq!(settings.format_options.quote_style, QuoteStyle::Single);
    }

    #[test]
    fn maps_all_formatter_options() {
        let json = r#"{ "format":{ "block_newline_gaps":"preserve","sort_requires":true,
            "space_after_function_names":"calls","magic_trailing_comma":true,"call_parentheses":"input" } }"#;
        let c = luck_core::config::parse_luck_config(json).unwrap();
        let o = format_options_from_config(&c);
        assert_eq!(o.block_newline_gaps, BlockNewlineGaps::Preserve);
        assert!(o.sort_requires);
        assert_eq!(o.space_after_function_names, SpaceAfterFunction::Calls);
        assert!(o.magic_trailing_comma);
        assert_eq!(o.call_parentheses, CallParentheses::Input);
    }

    #[test]
    fn resolved_format_options_picks_up_editorconfig() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write editorconfig");
        let file_path = dir.path().join("main.lua");

        // No project format config: the `.editorconfig` is the only source.
        let settings = ProjectSettings::default();
        let options = resolved_format_options(&settings, Some(&file_path));
        assert_eq!(options.indent_style, IndentStyle::Spaces);
        assert_eq!(options.indent_width, 2);
    }

    #[test]
    fn resolved_format_options_luck_json_overrides_editorconfig() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "[*.lua]\nindent_style = space\nindent_size = 2\n",
        )
        .expect("write editorconfig");
        let file_path = dir.path().join("main.lua");

        // luck.json `format` sets indent_width to 8, which must win over the
        // `.editorconfig` value of 2.
        let config =
            luck_core::config::parse_luck_config(r#"{ "format": { "indent_width": 8 } }"#).unwrap();
        let settings = build_settings(&config, None);
        let options = resolved_format_options(&settings, Some(&file_path));
        assert_eq!(options.indent_width, 8);
        // indent_style not set in luck.json, so it still comes from editorconfig.
        assert_eq!(options.indent_style, IndentStyle::Spaces);
    }

    #[test]
    fn resolved_format_options_no_path_uses_precomputed() {
        let config =
            luck_core::config::parse_luck_config(r#"{ "format": { "indent_width": 7 } }"#).unwrap();
        let settings = build_settings(&config, None);
        let options = resolved_format_options(&settings, None);
        assert_eq!(options.indent_width, 7);
    }
}
