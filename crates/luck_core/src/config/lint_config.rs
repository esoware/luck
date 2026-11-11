//! They live here, not in `luck_linter`, so `luck.json` deserializes without
//! depending on the linter. `luck_linter` re-exports them.

use alloc::collections::BTreeMap;
use core::fmt;
use core::marker::PhantomData;

use serde::de::{
    Deserialize, Deserializer, Error, IgnoredAny, IntoDeserializer as _, SeqAccess, Visitor,
};

use super::luck_config::concat;
use crate::diagnostic::DiagnosticSeverity;

/// Linter settings from luck.json.
// `Serialize` prints it back in the shape it parses, for `--print-config`.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct LintConfig {
    /// A level per rule category. `"warn"` and `"error"` run every rule in the
    /// category at that severity, and `"off"` runs none of them. A category
    /// left out keeps its default: `correctness` runs each rule at the rule's
    /// own severity, and the others are off.
    #[serde(default)]
    pub categories: CategoryLevels,
    /// A setting per rule, keyed by rule name: `"off"`, `"warn"`, or
    /// `"error"`, or `[level, options]` for a rule that takes options. It
    /// wins over the rule's category.
    #[serde(default)]
    pub rules: RuleSettings,
    /// Extra global names to treat as defined, such as `vim` or `love`.
    #[serde(default)]
    pub extra_globals: Vec<String>,
}

impl LintConfig {
    /// `extra_globals` concatenates base-first without repeats, and a
    /// category or rule that `self` sets replaces the base's level for it. A
    /// bare level keeps the base's options, as `-D` does, and options from
    /// both sides merge: `paths` concatenate and `self`'s `max` wins.
    pub(super) fn merge_onto(self, base: Self) -> Self {
        Self {
            categories: self.categories.merge_onto(base.categories),
            rules: self.rules.merge_onto(base.rules),
            extra_globals: concat(self.extra_globals, base.extra_globals),
        }
    }
}

/// Whether a rule runs, and the severity of what it reports.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RuleLevel {
    Off,
    Warn,
    Error,
}

impl RuleLevel {
    /// `None` for [`RuleLevel::Off`].
    #[must_use]
    pub const fn severity(self) -> Option<DiagnosticSeverity> {
        match self {
            Self::Off => None,
            Self::Warn => Some(DiagnosticSeverity::Warning),
            Self::Error => Some(DiagnosticSeverity::Error),
        }
    }
}

/// A group of lint rules, which `categories` sets the level of together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Outright wrong or useless code.
    Correctness,
    /// Most likely wrong or useless.
    Suspicious,
    /// Idiomatic code style.
    Style,
    /// Performance improvements.
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

/// The level of each rule category that sets one.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
#[schemars(transform = with_perf_alias)]
pub struct CategoryLevels {
    /// Outright wrong or useless code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correctness: Option<RuleLevel>,
    /// Most likely wrong or useless.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspicious: Option<RuleLevel>,
    /// Idiomatic code style.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<RuleLevel>,
    /// Performance improvements.
    #[serde(alias = "perf", skip_serializing_if = "Option::is_none")]
    pub performance: Option<RuleLevel>,
}

impl CategoryLevels {
    #[must_use]
    pub const fn level(self, category: Category) -> Option<RuleLevel> {
        match category {
            Category::Correctness => self.correctness,
            Category::Suspicious => self.suspicious,
            Category::Style => self.style,
            Category::Performance => self.performance,
        }
    }

    pub const fn set_level(&mut self, category: Category, level: RuleLevel) {
        let slot = match category {
            Category::Correctness => &mut self.correctness,
            Category::Suspicious => &mut self.suspicious,
            Category::Style => &mut self.style,
            Category::Performance => &mut self.performance,
        };
        *slot = Some(level);
    }

    fn merge_onto(self, base: Self) -> Self {
        Self {
            correctness: self.correctness.or(base.correctness),
            suspicious: self.suspicious.or(base.suspicious),
            style: self.style.or(base.style),
            performance: self.performance.or(base.performance),
        }
    }
}

/// schemars drops serde aliases, and the schema denies unknown keys.
fn with_perf_alias(schema: &mut schemars::Schema) {
    if let Some(properties) =
        schema.get_mut("properties").and_then(|properties| properties.as_object_mut())
        && let Some(performance) = properties.get("performance").cloned()
    {
        properties.insert("perf".to_owned(), performance);
    }
}

/// A level per rule, keyed by rule name. Only the rules listed here take
/// options.
// A rule with options has its own field, so options on any other rule, or of
// the wrong shape, fail to deserialize.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
pub struct RuleSettings {
    /// Reports nothing until its options set `max`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cyclomatic_complexity: Option<RuleSetting<CyclomaticComplexityOptions>>,
    /// Reports nothing until its options list `paths`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restricted_module_paths: Option<RuleSetting<RestrictedModulePathsOptions>>,
    /// Every rule that takes no options, keyed by rule name.
    #[serde(flatten)]
    pub levels: BTreeMap<String, RuleLevel>,
}

impl RuleSettings {
    /// The level set for the rule named `name`, if any.
    #[must_use]
    pub fn level(&self, name: &str) -> Option<RuleLevel> {
        match name {
            "cyclomatic_complexity" => self.cyclomatic_complexity.as_ref().map(RuleSetting::level),
            "restricted_module_paths" => {
                self.restricted_module_paths.as_ref().map(RuleSetting::level)
            }
            _ => self.levels.get(name).copied(),
        }
    }

    /// Sets the level of the rule named `name` and keeps any options it has.
    pub fn set_level(&mut self, name: &str, level: RuleLevel) {
        match name {
            "cyclomatic_complexity" => set_setting_level(&mut self.cyclomatic_complexity, level),
            "restricted_module_paths" => {
                set_setting_level(&mut self.restricted_module_paths, level);
            }
            _ => {
                self.levels.insert(name.to_owned(), level);
            }
        }
    }

    /// Every rule name with a setting.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let cyclomatic_complexity =
            self.cyclomatic_complexity.as_ref().map(|_| "cyclomatic_complexity");
        let restricted_module_paths =
            self.restricted_module_paths.as_ref().map(|_| "restricted_module_paths");
        cyclomatic_complexity
            .into_iter()
            .chain(restricted_module_paths)
            .chain(self.levels.keys().map(String::as_str))
    }

    fn merge_onto(self, mut base: Self) -> Self {
        base.levels.extend(self.levels);
        Self {
            cyclomatic_complexity: merge_setting(
                self.cyclomatic_complexity,
                base.cyclomatic_complexity,
                |options, _| options,
            ),
            restricted_module_paths: merge_setting(
                self.restricted_module_paths,
                base.restricted_module_paths,
                |options, base_options| RestrictedModulePathsOptions {
                    paths: concat(options.paths, base_options.paths),
                },
            ),
            levels: base.levels,
        }
    }
}

fn set_setting_level<Options>(setting: &mut Option<RuleSetting<Options>>, level: RuleLevel) {
    *setting = Some(match setting.take() {
        Some(RuleSetting::WithOptions(_, options)) => RuleSetting::WithOptions(level, options),
        Some(RuleSetting::Level(_)) | None => RuleSetting::Level(level),
    });
}

fn merge_setting<Options>(
    child: Option<RuleSetting<Options>>,
    base: Option<RuleSetting<Options>>,
    merge_options: impl FnOnce(Options, Options) -> Options,
) -> Option<RuleSetting<Options>> {
    match (child, base) {
        (None, base_setting) => base_setting,
        (Some(RuleSetting::Level(level)), mut base_setting) => {
            set_setting_level(&mut base_setting, level);
            base_setting
        }
        (
            Some(RuleSetting::WithOptions(level, options)),
            Some(RuleSetting::WithOptions(_, base_options)),
        ) => Some(RuleSetting::WithOptions(level, merge_options(options, base_options))),
        (
            child_setting @ Some(RuleSetting::WithOptions(..)),
            Some(RuleSetting::Level(_)) | None,
        ) => child_setting,
    }
}

/// One rule's entry in `rules`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "RuleSetting_{Options}")]
pub enum RuleSetting<Options> {
    /// `"off"`, `"warn"`, or `"error"`.
    Level(RuleLevel),
    /// A level and the rule's options: `[level, options]`.
    WithOptions(RuleLevel, Options),
}

impl<Options> RuleSetting<Options> {
    #[must_use]
    pub const fn level(&self) -> RuleLevel {
        match self {
            Self::Level(level) | Self::WithOptions(level, _) => *level,
        }
    }

    #[must_use]
    pub const fn options(&self) -> Option<&Options> {
        match self {
            Self::Level(_) => None,
            Self::WithOptions(_, options) => Some(options),
        }
    }
}

// Written by hand because an untagged derive reports every mistake as "data
// did not match any variant", without the bad level or option key.
impl<'de, Options> Deserialize<'de> for RuleSetting<Options>
where
    Options: Deserialize<'de>,
{
    fn deserialize<Source>(deserializer: Source) -> Result<Self, Source::Error>
    where
        Source: Deserializer<'de>,
    {
        deserializer.deserialize_any(RuleSettingVisitor(PhantomData))
    }
}

struct RuleSettingVisitor<Options>(PhantomData<Options>);

impl<'de, Options> Visitor<'de> for RuleSettingVisitor<Options>
where
    Options: Deserialize<'de>,
{
    type Value = RuleSetting<Options>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a level or `[level, options]`")
    }

    fn visit_str<Failure>(self, v: &str) -> Result<Self::Value, Failure>
    where
        Failure: Error,
    {
        RuleLevel::deserialize(v.into_deserializer()).map(RuleSetting::Level)
    }

    fn visit_seq<Access>(self, mut seq: Access) -> Result<Self::Value, Access::Error>
    where
        Access: SeqAccess<'de>,
    {
        let level = seq.next_element()?.ok_or_else(|| Error::invalid_length(0, &self))?;
        let options = seq.next_element()?.ok_or_else(|| Error::invalid_length(1, &self))?;
        if seq.next_element::<IgnoredAny>()?.is_some() {
            return Err(Error::invalid_length(3, &self));
        }
        Ok(RuleSetting::WithOptions(level, options))
    }
}

/// Options of the `cyclomatic_complexity` rule.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct CyclomaticComplexityOptions {
    /// Complexity above which a function is flagged.
    pub max: u32,
}

/// Options of the `restricted_module_paths` rule.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct RestrictedModulePathsOptions {
    /// Modules that can't be required. A trailing `.*` also covers
    /// submodules: `forbidden.*` matches `forbidden` and `forbidden.sub`.
    pub paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_luck_config;

    fn lint_config(lint_json: &str) -> LintConfig {
        parse_luck_config(&format!(r#"{{ "lint": {lint_json} }}"#))
            .expect("deserialize")
            .lint
            .expect("lint present")
    }

    fn lint_error(lint_json: &str) -> String {
        parse_luck_config(&format!(r#"{{ "lint": {lint_json} }}"#)).unwrap_err()
    }

    const CATEGORIES: [Category; 4] =
        [Category::Correctness, Category::Suspicious, Category::Style, Category::Performance];

    #[test]
    fn parses_every_lint_option() {
        let lint = lint_config(
            r#"{
                "categories": { "correctness": "off", "style": "warn", "suspicious": "error" },
                "rules": {
                    "unused_variable": "off",
                    "undefined_variable": "error",
                    "cyclomatic_complexity": ["warn", { "max": 15 }],
                    "restricted_module_paths": ["error", { "paths": ["secret.path"] }]
                },
                "extra_globals": ["vim", "roblox"]
            }"#,
        );
        assert_eq!(lint.extra_globals, vec!["vim".to_owned(), "roblox".to_owned()]);
        assert_eq!(
            lint.categories,
            CategoryLevels {
                correctness: Some(RuleLevel::Off),
                suspicious: Some(RuleLevel::Error),
                style: Some(RuleLevel::Warn),
                performance: None,
            }
        );
        assert_eq!(lint.rules.level("unused_variable"), Some(RuleLevel::Off));
        assert_eq!(lint.rules.level("undefined_variable"), Some(RuleLevel::Error));
        assert_eq!(
            lint.rules.cyclomatic_complexity,
            Some(RuleSetting::WithOptions(
                RuleLevel::Warn,
                CyclomaticComplexityOptions { max: 15 }
            ))
        );
        assert_eq!(
            lint.rules.restricted_module_paths,
            Some(RuleSetting::WithOptions(
                RuleLevel::Error,
                RestrictedModulePathsOptions { paths: vec!["secret.path".to_owned()] }
            ))
        );
    }

    /// `--print-config` prints this, so it has to paste back into `luck.json`.
    #[test]
    fn serializes_back_to_the_shape_it_parses() {
        let lint = lint_config(
            r#"{
                "categories": { "perf": "warn" },
                "rules": {
                    "unused_variable": "off",
                    "cyclomatic_complexity": ["warn", { "max": 15 }],
                    "restricted_module_paths": "error"
                },
                "extra_globals": ["vim"]
            }"#,
        );
        let json = serde_json::to_string(&lint).expect("serialize");
        assert_eq!(
            json,
            r#"{"categories":{"performance":"warn"},"rules":{"cyclomatic_complexity":["warn",{"max":15}],"restricted_module_paths":"error","unused_variable":"off"},"extra_globals":["vim"]}"#
        );
        assert_eq!(lint_config(&json), lint);
    }

    #[test]
    fn leaves_unset_lint_options_empty() {
        let lint = lint_config("{}");
        assert_eq!(lint.categories, CategoryLevels::default());
        assert_eq!(lint.rules, RuleSettings::default());
        assert_eq!(lint.extra_globals, Vec::<String>::new());
    }

    #[test]
    fn a_rule_with_options_also_takes_a_bare_level() {
        let lint = lint_config(r#"{ "rules": { "cyclomatic_complexity": "warn" } }"#);
        assert_eq!(lint.rules.cyclomatic_complexity, Some(RuleSetting::Level(RuleLevel::Warn)));
        assert_eq!(lint.rules.level("cyclomatic_complexity"), Some(RuleLevel::Warn));
        assert!(lint.rules.levels.is_empty(), "{:?}", lint.rules.levels);
    }

    #[test]
    fn rejects_an_unknown_level() {
        for lint_json in [
            r#"{ "rules": { "undefined_variable": "fatal" } }"#,
            r#"{ "rules": { "cyclomatic_complexity": "fatal" } }"#,
            r#"{ "rules": { "cyclomatic_complexity": ["fatal", { "max": 1 }] } }"#,
            r#"{ "categories": { "style": "fatal" } }"#,
        ] {
            let error = lint_error(lint_json);
            assert!(error.contains("fatal"), "error should name the offending value: {error}");
        }
    }

    #[test]
    fn rejects_options_on_a_rule_that_takes_none() {
        let error = lint_error(r#"{ "rules": { "unused_variable": ["warn", { "max": 1 }] } }"#);
        assert!(error.contains("sequence"), "{error}");
    }

    #[test]
    fn rejects_an_unknown_rule_option() {
        let error =
            lint_error(r#"{ "rules": { "cyclomatic_complexity": ["warn", { "mx": 1 }] } }"#);
        assert!(error.contains("mx"), "error should name the offending key: {error}");
    }

    #[test]
    fn rejects_a_setting_list_of_the_wrong_length() {
        for setting in ["[]", r#"["warn"]"#, r#"["warn", { "max": 1 }, 2]"#] {
            let error =
                lint_error(&format!(r#"{{ "rules": {{ "cyclomatic_complexity": {setting} }} }}"#));
            assert!(error.contains("invalid length"), "{setting}: {error}");
        }
    }

    #[test]
    fn rejects_an_unknown_category() {
        let error = lint_error(r#"{ "categories": { "nursery": "warn" } }"#);
        assert!(error.contains("nursery"), "error should name the offending key: {error}");
    }

    #[test]
    fn parses_the_perf_category_alias() {
        let lint = lint_config(r#"{ "categories": { "perf": "warn" } }"#);
        assert_eq!(lint.categories.level(Category::Performance), Some(RuleLevel::Warn));
    }

    /// `name` is what `--rules` and `--print-config` print, so a `name` that
    /// `from_name` doesn't take back would break scripts feeding them to `-A`.
    #[test]
    fn category_names_round_trip_through_from_name() {
        for category in CATEGORIES {
            assert_eq!(Category::from_name(category.name()), Some(category));
        }
    }

    #[test]
    fn category_from_name_recognizes_categories_and_the_perf_alias() {
        assert_eq!(Category::from_name("style"), Some(Category::Style));
        assert_eq!(Category::from_name("perf"), Some(Category::Performance));
        assert_eq!(Category::from_name("undefined_variable"), None);
    }

    /// A category's `luck.json` key is its `name`, so `--print-config` output
    /// pastes back into `categories`.
    #[test]
    fn category_levels_keys_are_the_category_names() {
        for category in CATEGORIES {
            let lint = lint_config(&format!(
                r#"{{ "categories": {{ "{}": "error" }} }}"#,
                category.name()
            ));
            assert_eq!(lint.categories.level(category), Some(RuleLevel::Error));

            let mut levels = CategoryLevels::default();
            levels.set_level(category, RuleLevel::Warn);
            assert_eq!(levels.level(category), Some(RuleLevel::Warn));
        }
    }

    #[test]
    fn set_level_keeps_the_options_of_a_rule() {
        let mut rules =
            lint_config(r#"{ "rules": { "cyclomatic_complexity": ["warn", { "max": 4 }] } }"#)
                .rules;
        rules.set_level("cyclomatic_complexity", RuleLevel::Error);
        rules.set_level("restricted_module_paths", RuleLevel::Off);
        rules.set_level("unused_variable", RuleLevel::Warn);
        assert_eq!(
            rules.cyclomatic_complexity,
            Some(RuleSetting::WithOptions(
                RuleLevel::Error,
                CyclomaticComplexityOptions { max: 4 }
            ))
        );
        assert_eq!(rules.restricted_module_paths, Some(RuleSetting::Level(RuleLevel::Off)));
        assert_eq!(rules.level("unused_variable"), Some(RuleLevel::Warn));
        assert_eq!(
            rules.names().collect::<Vec<_>>(),
            ["cyclomatic_complexity", "restricted_module_paths", "unused_variable"]
        );
    }

    #[test]
    fn merge_onto_lets_the_child_replace_a_category_or_rule() {
        let base = lint_config(
            r#"{
                "categories": { "style": "warn", "correctness": "off" },
                "rules": {
                    "empty_block": "warn",
                    "unused_variable": "off",
                    "cyclomatic_complexity": ["warn", { "max": 10 }]
                }
            }"#,
        );
        let child = lint_config(
            r#"{
                "categories": { "style": "error", "performance": "warn" },
                "rules": { "unused_variable": "error", "cyclomatic_complexity": "off" }
            }"#,
        );
        let lint = child.merge_onto(base);
        assert_eq!(
            lint.categories,
            CategoryLevels {
                correctness: Some(RuleLevel::Off),
                suspicious: None,
                style: Some(RuleLevel::Error),
                performance: Some(RuleLevel::Warn),
            }
        );
        assert_eq!(lint.rules.level("empty_block"), Some(RuleLevel::Warn));
        assert_eq!(lint.rules.level("unused_variable"), Some(RuleLevel::Error));
        assert_eq!(
            lint.rules.cyclomatic_complexity,
            Some(RuleSetting::WithOptions(RuleLevel::Off, CyclomaticComplexityOptions { max: 10 }))
        );
    }

    /// A child that only raises a rule's severity mustn't silence it by
    /// dropping the base's options.
    #[test]
    fn merge_onto_keeps_the_base_options_and_merges_the_child_s() {
        let base = lint_config(
            r#"{
                "rules": {
                    "cyclomatic_complexity": ["warn", { "max": 10 }],
                    "restricted_module_paths": ["warn", { "paths": ["secret.*"] }]
                }
            }"#,
        );
        let raised = lint_config(r#"{ "rules": { "cyclomatic_complexity": "error" } }"#);
        assert_eq!(
            raised.merge_onto(base.clone()).rules.cyclomatic_complexity,
            Some(RuleSetting::WithOptions(
                RuleLevel::Error,
                CyclomaticComplexityOptions { max: 10 }
            ))
        );

        let extended = lint_config(
            r#"{
                "rules": {
                    "cyclomatic_complexity": ["warn", { "max": 4 }],
                    "restricted_module_paths": ["error", { "paths": ["legacy", "secret.*"] }]
                }
            }"#,
        );
        let rules = extended.merge_onto(base).rules;
        assert_eq!(
            rules.cyclomatic_complexity,
            Some(RuleSetting::WithOptions(RuleLevel::Warn, CyclomaticComplexityOptions { max: 4 }))
        );
        assert_eq!(
            rules.restricted_module_paths,
            Some(RuleSetting::WithOptions(
                RuleLevel::Error,
                RestrictedModulePathsOptions {
                    paths: vec!["secret.*".to_owned(), "legacy".to_owned()]
                }
            ))
        );
    }

    #[test]
    fn schema_takes_the_perf_category_alias() {
        let schema = serde_json::to_value(schemars::schema_for!(CategoryLevels)).expect("schema");
        assert_eq!(schema["properties"]["perf"], schema["properties"]["performance"]);
    }
}
