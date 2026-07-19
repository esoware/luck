use crate::diagnostics::{Category, DiagnosticSeverity};
use crate::format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, HexCase, IndentStyle, LineEndings,
    QuoteStyle, SpaceAfterFunction,
};
use crate::transform_config::TransformConfig;
use crate::types::LuaTarget;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const CONFIG_FILE_NAME: &str = "luck.json";

/// A single entry point in a multi-entry build configuration.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryConfig {
    pub entry: String,
    pub output: String,
}

/// Settings that a named profile can override (e.g. `"release"`).
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileOverrides {
    pub minify: Option<bool>,
    pub transforms: Option<TransformConfig>,
}

/// Per-rule override slot. `enabled` toggles a rule on/off; `severity`
/// overrides the default severity (`"error"` or `"warning"`). Both are
/// optional so the user can override one without touching the other.
///
/// This is the single lint rule-override type; `luck_linter` re-exports it.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleSetting {
    pub enabled: Option<bool>,
    pub severity: Option<DiagnosticSeverity>,
}

/// Linter settings from luck.json. This is the single source of truth for
/// lint configuration; `luck_linter` re-exports it and reads it directly.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LintConfig {
    /// Per-rule overrides, keyed by rule name.
    #[serde(default)]
    pub rule_overrides: HashMap<String, RuleSetting>,
    /// Additional allowed global names (e.g. `vim`, `roblox`).
    #[serde(default)]
    pub extra_globals: Vec<String>,
    /// Module paths whose `require` is restricted by the
    /// `restricted_module_paths` rule.
    #[serde(default)]
    pub restricted_module_paths: Vec<String>,
    /// Maximum cyclomatic complexity threshold for the
    /// `cyclomatic_complexity` rule.
    #[serde(default)]
    pub max_cyclomatic_complexity: Option<u32>,
    /// When true, only rules explicitly enabled via `rule_overrides`
    /// run - even correctness defaults are silenced.
    #[serde(default)]
    pub disable_default_rules: bool,
    /// Rule categories to enable as a group (e.g. `suspicious`, `style`).
    #[serde(default)]
    pub categories: Vec<Category>,
}

/// Formatter settings from luck.json.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormatConfig {
    pub line_width: Option<u16>,
    pub indent_style: Option<IndentStyle>,
    pub indent_width: Option<u8>,
    pub quote_style: Option<QuoteStyle>,
    /// `"preserve"` (default) / `"lower"` / `"upper"`. Case of hex digits
    /// `A`-`F` in numeric literals; the `0x` prefix is always lowercased.
    pub hexadecimal_case: Option<HexCase>,
    pub call_parentheses: Option<CallParentheses>,
    pub collapse_simple_statement: Option<CollapseSimpleStatement>,
    pub line_endings: Option<LineEndings>,
    /// `"never"` (default) or `"preserve"`. Controls whether blank lines at
    /// the start/end of block bodies are preserved verbatim.
    pub block_newline_gaps: Option<BlockNewlineGaps>,
    /// When true, consecutive `local NAME = require(...)` statements at the
    /// top of a block are sorted alphabetically by name.
    pub sort_requires: Option<bool>,
    /// `"never"` (default) / `"definitions"` / `"calls"` / `"always"`.
    /// Inserts a space before `(` in function definitions, calls, or both.
    pub space_after_function_names: Option<SpaceAfterFunction>,
    /// When true, a trailing comma in a table or call argument list forces
    /// the surrounding group to break across multiple lines (Black/Prettier
    /// convention). Defaults to `false` to preserve existing pack/hug behavior.
    pub magic_trailing_comma: Option<bool>,
}

/// Top-level luck.json configuration.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LuckConfig {
    pub lua: Option<String>,
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
    pub search_paths: Option<Vec<String>>,
    pub transforms: Option<TransformConfig>,
    pub profiles: Option<HashMap<String, ProfileOverrides>>,
    pub preamble: Option<String>,
    pub luck_preamble: Option<bool>,
    pub format: Option<FormatConfig>,
    pub lint: Option<LintConfig>,
}

impl LuckConfig {
    /// Resolves the target for `.lua` files. Defaults to Lua 5.4. The `lua`
    /// key must name a Lua 5.x dialect - naming a Luau dialect is an error.
    pub fn lua_target(&self) -> Result<LuaTarget, String> {
        match &self.lua {
            Some(value) => {
                let target: LuaTarget = value.parse()?;
                if target.is_luau() {
                    return Err(format!("\"lua\" must be lua51..lua55, got \"{value}\""));
                }
                Ok(target)
            }
            None => Ok(LuaTarget::Lua54),
        }
    }

    /// Resolves the target for `.luau` files. Defaults to standalone Luau. The
    /// `luau` key must name a Luau dialect - naming a Lua 5.x dialect is an error.
    pub fn luau_target(&self) -> Result<LuaTarget, String> {
        match &self.luau {
            Some(value) => {
                let target: LuaTarget = value.parse()?;
                if !target.is_luau() {
                    return Err(format!(
                        "\"luau\" must be \"luau\" or \"roblox\", got \"{value}\""
                    ));
                }
                Ok(target)
            }
            None => Ok(LuaTarget::Luau),
        }
    }

    /// Selects the target by file extension: `.luau` uses [`luau_target`],
    /// everything else uses [`lua_target`].
    pub fn target_for_path(&self, path: &Path) -> Result<LuaTarget, String> {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("luau") => self.luau_target(),
            _ => self.lua_target(),
        }
    }

    /// Merge `self` (more specific) on top of `base`. Scalars/Options: self
    /// wins. Vecs: base then self (concatenated). Maps & nested config:
    /// deep-merged with self winning per key.
    pub fn merge_onto(self, base: LuckConfig) -> LuckConfig {
        fn cat<T>(child: Option<Vec<T>>, base: Option<Vec<T>>) -> Option<Vec<T>> {
            match (child, base) {
                (Some(child), Some(mut base)) => {
                    base.extend(child);
                    Some(base)
                }
                (child, base) => child.or(base),
            }
        }
        LuckConfig {
            lua: self.lua.or(base.lua),
            luau: self.luau.or(base.luau),
            root: self.root.or(base.root),
            extends: None, // already resolved; do not re-expand
            include: cat(self.include, base.include),
            exclude: cat(self.exclude, base.exclude),
            entry: self.entry.or(base.entry),
            output_dir: self.output_dir.or(base.output_dir),
            output: self.output.or(base.output),
            entries: self.entries.or(base.entries),
            minify: self.minify.or(base.minify),
            search_paths: cat(self.search_paths, base.search_paths),
            transforms: self.transforms.or(base.transforms),
            profiles: self.profiles.or(base.profiles),
            preamble: self.preamble.or(base.preamble),
            luck_preamble: self.luck_preamble.or(base.luck_preamble),
            format: merge_format(self.format, base.format),
            lint: merge_lint(self.lint, base.lint),
        }
    }
}

/// Merge child `FormatConfig` over base, field-by-field (child wins per field).
pub(crate) fn merge_format(
    child: Option<FormatConfig>,
    base: Option<FormatConfig>,
) -> Option<FormatConfig> {
    match (child, base) {
        (Some(child), Some(base)) => Some(FormatConfig {
            line_width: child.line_width.or(base.line_width),
            indent_style: child.indent_style.or(base.indent_style),
            indent_width: child.indent_width.or(base.indent_width),
            quote_style: child.quote_style.or(base.quote_style),
            hexadecimal_case: child.hexadecimal_case.or(base.hexadecimal_case),
            call_parentheses: child.call_parentheses.or(base.call_parentheses),
            collapse_simple_statement: child
                .collapse_simple_statement
                .or(base.collapse_simple_statement),
            line_endings: child.line_endings.or(base.line_endings),
            block_newline_gaps: child.block_newline_gaps.or(base.block_newline_gaps),
            sort_requires: child.sort_requires.or(base.sort_requires),
            space_after_function_names: child
                .space_after_function_names
                .or(base.space_after_function_names),
            magic_trailing_comma: child.magic_trailing_comma.or(base.magic_trailing_comma),
        }),
        (child, base) => child.or(base),
    }
}

/// Merge child `LintConfig` over base: Vec fields concatenate (base then
/// child), `rule_overrides` deep-merges (child entries overwrite by key),
/// `max_cyclomatic_complexity` is child-or-base. `disable_default_rules` is a
/// lockdown flag, so it is ORed: a base that disables defaults stays disabled
/// even if a child omits the flag (a bare `bool` can't tell "omitted" from
/// "false", and silently re-enabling a parent's disabled defaults is the more
/// dangerous failure).
fn merge_lint(child: Option<LintConfig>, base: Option<LintConfig>) -> Option<LintConfig> {
    match (child, base) {
        (Some(child), Some(mut base)) => {
            base.extra_globals.extend(child.extra_globals);
            base.restricted_module_paths
                .extend(child.restricted_module_paths);
            base.rule_overrides.extend(child.rule_overrides);
            base.categories.extend(child.categories);
            LintConfig {
                rule_overrides: base.rule_overrides,
                extra_globals: base.extra_globals,
                restricted_module_paths: base.restricted_module_paths,
                max_cyclomatic_complexity: child
                    .max_cyclomatic_complexity
                    .or(base.max_cyclomatic_complexity),
                disable_default_rules: child.disable_default_rules || base.disable_default_rules,
                categories: base.categories,
            }
            .into()
        }
        (child, base) => child.or(base),
    }
}

/// Decides whether a file is part of the project, from include/exclude
/// globs relative to the config directory. Exclude wins over include.
#[derive(Debug, Clone)]
pub struct ProjectFilter {
    base: PathBuf,
    include: GlobSet,
    exclude: GlobSet,
}

impl ProjectFilter {
    pub fn new(
        base: &Path,
        include: &Option<Vec<String>>,
        exclude: &Option<Vec<String>>,
    ) -> Result<Self, String> {
        // `{**/,}` matches files at the project root and in any subdirectory,
        // since `**/` alone does not match zero path segments.
        let default_include = vec!["{**/,}*.lua".to_string(), "{**/,}*.luau".to_string()];
        let inc = include.clone().unwrap_or(default_include);
        // Canonicalize once: callers canonicalize candidate files (on
        // Windows that yields `\\?\C:\...`), and strip_prefix against a
        // non-canonical base silently fails - every include matched
        // nothing and every exclude excluded nothing.
        let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
        Ok(Self {
            base,
            include: build_set(&inc)?,
            exclude: build_set(exclude.as_deref().unwrap_or(&[]))?,
        })
    }

    /// True if `path` is inside the project and matches include but not exclude.
    pub fn is_included(&self, path: &Path) -> bool {
        let canonical;
        let candidate = match path.canonicalize() {
            Ok(resolved) => {
                canonical = resolved;
                canonical.as_path()
            }
            Err(_) => path,
        };
        let rel = candidate.strip_prefix(&self.base).unwrap_or(candidate);
        self.include.is_match(rel) && !self.exclude.is_match(rel)
    }
}

fn build_set(patterns: &[String]) -> Result<GlobSet, String> {
    // Invalid patterns are hard errors, matching the config contract
    // everywhere else - a typo'd exclude silently processing generated
    // code is exactly the failure deny_unknown_fields exists to prevent.
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob =
            Glob::new(pattern).map_err(|e| format!("invalid glob pattern \"{pattern}\": {e}"))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| format!("invalid glob set: {e}"))
}

/// Parsed `.luaurc` file with optional aliases for Luau module resolution.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LuauRc {
    pub language_mode: Option<String>,
    pub aliases: Option<std::collections::HashMap<String, String>>,
}

/// Fully resolved configuration for a single build target, ready for execution.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub target: LuaTarget,
    pub entry: PathBuf,
    pub output: PathBuf,
    pub minify: bool,
    pub search_paths: Vec<String>,
    pub rc_dir: PathBuf,
    pub transforms: TransformConfig,
    pub preamble: Option<String>,
    pub luck_preamble: bool,
}

/// Default Lua 5.x search path templates when none are configured.
pub const DEFAULT_SEARCH_PATHS: &[&str] = &["?.lua", "?/init.lua"];

/// Deserializes a `luck.json` (JSON5) string into a [`LuckConfig`].
pub fn parse_luck_config(contents: &str) -> Result<LuckConfig, String> {
    json5::from_str(contents)
        .map_err(|e| format!("Failed to parse {CONFIG_FILE_NAME} (JSON5): {e}"))
}

/// Loads a config file and recursively applies its `extends` chain, merging
/// each parent base-first. Cycles are detected and errored.
pub fn load_with_extends(path: &Path) -> Result<LuckConfig, String> {
    let mut visited = std::collections::HashSet::new();
    load_with_extends_inner(path, &mut visited)
}

fn load_with_extends_inner(
    path: &Path,
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Result<LuckConfig, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // `visited` tracks the ACTIVE chain, not everything ever loaded - a
    // diamond (A extends B and C, both extend D) is legal; only a path
    // back onto the current chain is a cycle. Popped before returning.
    if !visited.insert(canonical.clone()) {
        return Err(format!("circular extends chain at {}", path.display()));
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config = parse_luck_config(&contents)?;
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut merged = LuckConfig::default();
    if let Some(parents) = &config.extends {
        // A `root: true` config is the top of its project; its extends must
        // not escape its own directory subtree.
        let root_boundary = if config.root == Some(true) {
            Some(
                dir.canonicalize()
                    .map_err(|e| format!("{}: {e}", dir.display()))?,
            )
        } else {
            None
        };
        for rel in parents {
            let parent_path = dir.join(rel);
            if let Some(root_dir) = &root_boundary {
                let target = parent_path
                    .canonicalize()
                    .map_err(|e| format!("{}: {e}", parent_path.display()))?;
                if !target.starts_with(root_dir) {
                    return Err(format!(
                        "extends path \"{rel}\" escapes the root boundary at {}",
                        root_dir.display()
                    ));
                }
            }
            let parent = load_with_extends_inner(&parent_path, visited)?;
            merged = parent.merge_onto(merged); // later extends override earlier
        }
    }
    visited.remove(&canonical);
    Ok(config.merge_onto(merged)) // current file overrides all parents
}

/// Deserializes a `.luaurc` (JSON5) string into a [`LuauRc`].
pub fn parse_luaurc(contents: &str) -> Result<LuauRc, String> {
    json5::from_str(contents).map_err(|e| format!("Failed to parse .luaurc: {e}"))
}

/// Walks up from `start_dir` looking for a `luck.json` file.
pub fn discover_config(start_dir: &Path) -> Result<Option<(PathBuf, LuckConfig)>, String> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            let config = load_with_extends(&candidate)?;
            return Ok(Some((candidate, config)));
        }
        if !dir.pop() {
            return Ok(None);
        }
    }
}

/// Loads or discovers the config, applies profile overrides, and returns resolved build configs.
pub fn resolve_build_config(
    config_path: Option<&Path>,
    profile: Option<&str>,
) -> Result<Vec<BuildConfig>, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("Failed to get current directory: {e}"))?;

    let (rc_path, file_config) = if let Some(path) = config_path {
        let config = load_with_extends(path)?;
        (path.to_path_buf(), config)
    } else {
        match discover_config(&cwd)? {
            Some((path, config)) => (path, config),
            None => {
                return Err(format!(
                    "No {CONFIG_FILE_NAME} found. Run from a project directory or use --config."
                ));
            }
        }
    };

    let rc_dir = rc_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cwd.clone());

    let profile_overrides = if let Some(name) = profile {
        match &file_config.profiles {
            Some(profiles) => match profiles.get(name) {
                Some(p) => Some(p.clone()),
                None => {
                    return Err(format!(
                        "Profile \"{name}\" not found in {CONFIG_FILE_NAME}"
                    ));
                }
            },
            None => return Err(format!("No profiles defined in {CONFIG_FILE_NAME}")),
        }
    } else {
        None
    };

    let minify = profile_overrides
        .as_ref()
        .and_then(|p| p.minify)
        .or(file_config.minify)
        .unwrap_or(false);
    let transforms = profile_overrides
        .as_ref()
        .and_then(|p| p.transforms.clone())
        .or(file_config.transforms.clone())
        .unwrap_or_default();

    let search_paths = file_config
        .search_paths
        .clone()
        .unwrap_or_else(|| DEFAULT_SEARCH_PATHS.iter().map(|s| s.to_string()).collect());

    let preamble = file_config.preamble.clone();
    let luck_preamble = file_config.luck_preamble.unwrap_or(false);

    if file_config.entries.is_some() && file_config.entry.is_some() {
        return Err(format!(
            "Cannot specify both \"entry\" and \"entries\" in {CONFIG_FILE_NAME}"
        ));
    }

    if let Some(entries) = &file_config.entries {
        let mut configs = Vec::new();
        for ec in entries {
            let entry = rc_dir.join(&ec.entry);
            let target = file_config.target_for_path(&entry)?;
            if !entry.is_file() {
                return Err(format!("Entry file not found: {}", entry.display()));
            }
            let output = rc_dir.join(&ec.output);
            configs.push(BuildConfig {
                target,
                entry,
                output,
                minify,
                search_paths: search_paths.clone(),
                rc_dir: rc_dir.clone(),
                transforms: transforms.clone(),
                preamble: preamble.clone(),
                luck_preamble,
            });
        }
        Ok(configs)
    } else {
        let entry_str = file_config
            .entry
            .as_deref()
            .ok_or_else(|| format!("Missing \"entry\" in {CONFIG_FILE_NAME}"))?;
        let entry = rc_dir.join(entry_str);
        let target = file_config.target_for_path(&entry)?;
        if !entry.is_file() {
            return Err(format!("Entry file not found: {}", entry.display()));
        }

        let output = if let Some(out) = &file_config.output {
            rc_dir.join(out)
        } else {
            let output_dir_str = file_config.output_dir.as_deref().ok_or_else(|| {
                format!("Missing \"output_dir\" or \"output\" in {CONFIG_FILE_NAME}")
            })?;
            let output_dir = rc_dir.join(output_dir_str);
            let entry_filename = entry
                .file_name()
                .ok_or_else(|| "Entry path has no filename".to_string())?;
            output_dir.join(entry_filename)
        };

        Ok(vec![BuildConfig {
            target,
            entry,
            output,
            minify,
            search_paths,
            rc_dir,
            transforms,
            preamble,
            luck_preamble,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        use crate::Category;
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
        use crate::Category;
        let config = parse_luck_config(r#"{ "lint": { "categories": ["perf"] } }"#).unwrap();
        assert_eq!(config.lint.unwrap().categories, vec![Category::Performance]);
    }

    #[test]
    fn project_filter_includes_and_excludes() {
        let base = Path::new("/proj");
        let filter = ProjectFilter::new(
            base,
            &Some(vec!["src/**/*.lua".into(), "src/**/*.luau".into()]),
            &Some(vec!["**/gen/**".into()]),
        )
        .expect("valid globs");
        assert!(filter.is_included(Path::new("/proj/src/a.lua")));
        assert!(!filter.is_included(Path::new("/proj/src/gen/b.lua")));
        assert!(!filter.is_included(Path::new("/proj/other/c.lua")));
    }
    #[test]
    fn project_filter_default_includes_lua_luau() {
        let filter = ProjectFilter::new(Path::new("/proj"), &None, &None).expect("valid globs");
        assert!(filter.is_included(Path::new("/proj/x.lua")));
        assert!(filter.is_included(Path::new("/proj/sub/y.luau")));
        assert!(!filter.is_included(Path::new("/proj/x.txt")));
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
