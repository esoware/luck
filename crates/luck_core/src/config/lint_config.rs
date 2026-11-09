//! The `lint` section's types.
//!
//! They live here, not in `luck_linter`, so `luck.json` deserializes without
//! depending on the linter. `luck_linter` re-exports them.

use alloc::collections::BTreeMap;

use super::luck_config::concat;
use crate::diagnostic::DiagnosticSeverity;

/// Linter settings from luck.json.
#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
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

impl LintConfig {
    /// Merges `self` over `base`.
    ///
    /// Lists concatenate base-first without repeats, a `rule_overrides` entry
    /// from `self` replaces the base's entry for the same rule, and
    /// `max_cyclomatic_complexity` is self-or-base. `disable_default_rules` is
    /// true if either side sets it: a bare `bool` can't tell "omitted" from
    /// `false`, and silently re-enabling a parent's disabled defaults is the
    /// worse failure.
    pub(super) fn merge_onto(self, mut base: Self) -> Self {
        base.rule_overrides.extend(self.rule_overrides);
        Self {
            rule_overrides: base.rule_overrides,
            extra_globals: concat(self.extra_globals, base.extra_globals),
            restricted_module_paths: concat(
                self.restricted_module_paths,
                base.restricted_module_paths,
            ),
            max_cyclomatic_complexity: self
                .max_cyclomatic_complexity
                .or(base.max_cyclomatic_complexity),
            disable_default_rules: self.disable_default_rules || base.disable_default_rules,
            categories: concat(self.categories, base.categories),
        }
    }
}

/// Overrides for one lint rule.
///
/// `enabled` turns the rule on or off, and `severity` replaces its default
/// severity. Either can be set alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleSetting {
    pub enabled: Option<bool>,
    pub severity: Option<DiagnosticSeverity>,
}

/// A group of lint rules, enabled together through `categories`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, schemars::JsonSchema)]
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

impl Category {
    /// The name `luck.json`, `--rules`, and `--print-config` spell it with.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Correctness => "correctness",
            Self::Suspicious => "suspicious",
            Self::Style => "style",
            Self::Performance => "performance",
        }
    }

    /// Parses a `-A`/`-W`/`-D` flag value, case-insensitively, or `None` when
    /// it names no category.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "correctness" => Some(Self::Correctness),
            "suspicious" => Some(Self::Suspicious),
            "style" => Some(Self::Style),
            "performance" | "perf" => Some(Self::Performance),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_luck_config;

    #[test]
    fn parses_every_lint_option() {
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
                "disable_default_rules": true,
                "categories": ["style", "suspicious"]
            }
        }"#;
        let config = parse_luck_config(json).expect("deserialize");
        let lint = config.lint.expect("lint present");
        assert_eq!(lint.extra_globals, vec!["vim".to_owned(), "roblox".to_owned()]);
        assert_eq!(lint.restricted_module_paths, vec!["secret.path".to_owned()]);
        assert_eq!(lint.max_cyclomatic_complexity, Some(15));
        assert!(lint.disable_default_rules);
        assert_eq!(lint.categories, vec![Category::Style, Category::Suspicious]);
        let unused_variable =
            lint.rule_overrides.get("unused_variable").expect("unused_variable override");
        assert_eq!(unused_variable.enabled, Some(false));
        assert!(unused_variable.severity.is_none());
        let undefined_variable =
            lint.rule_overrides.get("undefined_variable").expect("undefined_variable override");
        assert_eq!(undefined_variable.severity, Some(DiagnosticSeverity::Error));
    }

    #[test]
    fn leaves_unset_lint_options_empty() {
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
        assert_eq!(lint.categories, Vec::<Category>::new());
    }

    #[test]
    fn rejects_an_unknown_severity() {
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
    fn parses_the_perf_category_alias() {
        let config = parse_luck_config(r#"{ "lint": { "categories": ["perf"] } }"#).unwrap();
        assert_eq!(config.lint.unwrap().categories, vec![Category::Performance]);
    }

    /// `name` is what `--rules` and `--print-config` print, so a `name` that
    /// `from_name` doesn't take back would break scripts feeding them to `-A`.
    #[test]
    fn category_names_round_trip_through_from_name() {
        for category in
            [Category::Correctness, Category::Suspicious, Category::Style, Category::Performance]
        {
            assert_eq!(Category::from_name(category.name()), Some(category));
        }
    }

    #[test]
    fn category_from_name_recognizes_categories_and_the_perf_alias() {
        assert_eq!(Category::from_name("style"), Some(Category::Style));
        assert_eq!(Category::from_name("perf"), Some(Category::Performance));
        assert_eq!(Category::from_name("undefined_variable"), None);
    }

    #[test]
    fn merge_onto_concatenates_categories_and_keeps_complexity_and_lockdown() {
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
