//! The deserialized `luck.json` surface and its merge semantics.
//!
//! Every type here derives `Deserialize` with `deny_unknown_fields` and
//! `schemars::JsonSchema`; the committed VS Code schema is generated from
//! [`LuckConfig`]. Merge (`extends` chains, profile layering) is pure
//! value-to-value transformation - no filesystem access lives here.

use crate::diagnostics::{Category, DiagnosticSeverity};
use crate::format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, HexCase, IndentStyle, LineEndings,
    QuoteStyle, SpaceAfterFunction,
};
use crate::transform_config::TransformConfig;
use crate::types::LuaTarget;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

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
    ///
    /// [`lua_target`]: LuckConfig::lua_target
    /// [`luau_target`]: LuckConfig::luau_target
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
