//! The `lint` section of `luck.json`. The linter owns its shape, since rule
//! names are its keys and each rule decides the options it takes.

#[cfg(feature = "schemars")]
use std::borrow::Cow;
#[cfg(feature = "schemars")]
use std::collections::BTreeMap;
#[cfg(feature = "serde")]
use std::fmt;
#[cfg(feature = "serde")]
use std::marker::PhantomData;

use luck_diagnostics::Severity;
use rustc_hash::FxHashSet;
#[cfg(feature = "schemars")]
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
#[cfg(feature = "serde")]
use serde::de::{
    Deserialize, DeserializeSeed, Deserializer, Error, IgnoredAny, IntoDeserializer as _,
    MapAccess, SeqAccess, Visitor,
};
#[cfg(feature = "serde")]
use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::diagnostic::Category;
use crate::rule::RuleOptions;
use crate::rules::{OptionSettings, RuleId, RuleTable};

/// Linter settings from luck.json.
// `Serialize` prints it back in the shape it parses, for `--print-config`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LintConfig {
    /// A level per rule category. `"warn"` and `"error"` run every rule in the
    /// category at that severity, and `"off"` runs none of them. A category
    /// left out keeps its default: `correctness` runs each rule at the rule's
    /// own severity, and the others are off.
    #[cfg_attr(feature = "serde", serde(default))]
    pub categories: CategoryLevels,
    /// A setting per rule, keyed by rule name: `"off"`, `"warn"`, or
    /// `"error"`, or `[level, options]` for a rule that takes options. It
    /// wins over the rule's category.
    #[cfg_attr(feature = "serde", serde(default))]
    pub rules: RuleSettings,
    /// Extra global names to treat as defined, such as `vim` or `love`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub extra_globals: Vec<String>,
}

impl LintConfig {
    /// `self` layered on `base`, as a `luck.json` that `extends` another
    /// takes its `lint` section.
    ///
    /// A category or rule `self` sets replaces the base's level for it. A
    /// bare level keeps the base's options, as `-D` does, and options from
    /// both sides merge: `paths` concatenate like `extra_globals`, base-first
    /// without repeats, and the child's `max` wins.
    #[must_use]
    pub fn merge_onto(self, base: Self) -> Self {
        let categories = CategoryLevels {
            correctness: self.categories.correctness.or(base.categories.correctness),
            suspicious: self.categories.suspicious.or(base.categories.suspicious),
            style: self.categories.style.or(base.categories.style),
            performance: self.categories.performance.or(base.categories.performance),
        };
        let rules = RuleSettings {
            levels: RuleTable::from_fn(|rule| self.rules.levels[rule].or(base.rules.levels[rule])),
            options: self.rules.options.merge_onto(base.rules.options),
        };
        Self { categories, rules, extra_globals: concat(self.extra_globals, base.extra_globals) }
    }

    /// The severity `rule` runs at, or `None` when it's off. The rule's own
    /// level wins, then its category's. With neither set, only correctness
    /// rules run, each at its own severity.
    pub(crate) fn severity(&self, rule: RuleId) -> Option<Severity> {
        let category = rule.category();
        self.rules.level(rule).or_else(|| self.categories.level(category)).map_or_else(
            || (category == Category::Correctness).then(|| rule.default_severity()),
            RuleLevel::severity,
        )
    }
}

/// `child` merged onto `base`, the options one rule takes from two layers.
pub(crate) fn merge_options<Options>(
    child: Option<Options>,
    base: Option<Options>,
) -> Option<Options>
where
    Options: RuleOptions,
{
    match (child, base) {
        (Some(child_options), Some(base_options)) => Some(child_options.merge_onto(base_options)),
        (child_options, base_options) => child_options.or(base_options),
    }
}

/// `base` followed by the items of `child` it lacks, so a file layered twice
/// through `extends` can't double a list.
pub(crate) fn concat(child: Vec<String>, mut base: Vec<String>) -> Vec<String> {
    let present: FxHashSet<&String> = base.iter().collect();
    let added: Vec<String> = child.into_iter().filter(|item| !present.contains(item)).collect();
    base.extend(added);
    base
}

/// Whether a rule runs, and the severity of what it reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum RuleLevel {
    Off,
    Warn,
    Error,
}

impl RuleLevel {
    /// `None` for [`RuleLevel::Off`].
    pub const fn severity(self) -> Option<Severity> {
        match self {
            Self::Off => None,
            Self::Warn => Some(Severity::Warning),
            Self::Error => Some(Severity::Error),
        }
    }
}

/// The level of each rule category that sets one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(transform = with_perf_alias))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct CategoryLevels {
    /// Outright wrong or useless code.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub correctness: Option<RuleLevel>,
    /// Most likely wrong or useless.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub suspicious: Option<RuleLevel>,
    /// Idiomatic code style.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub style: Option<RuleLevel>,
    /// Performance improvements.
    #[cfg_attr(feature = "serde", serde(alias = "perf", skip_serializing_if = "Option::is_none"))]
    pub performance: Option<RuleLevel>,
}

impl CategoryLevels {
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
}

/// schemars drops serde aliases, and the schema denies unknown keys.
#[cfg(feature = "schemars")]
fn with_perf_alias(schema: &mut Schema) {
    if let Some(properties) =
        schema.get_mut("properties").and_then(|properties| properties.as_object_mut())
        && let Some(performance) = properties.get("performance").cloned()
    {
        properties.insert("perf".to_owned(), performance);
    }
}

/// The level each rule's entry in `rules` sets, and the options it gives
/// one that takes options. A name no rule has is an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleSettings {
    levels: RuleTable<Option<RuleLevel>>,
    options: OptionSettings,
}

impl RuleSettings {
    /// The level set for `rule`, if any.
    pub fn level(&self, rule: RuleId) -> Option<RuleLevel> {
        self.levels[rule]
    }

    /// Sets the level of `rule` and keeps any options it has.
    pub fn set_level(&mut self, rule: RuleId, level: RuleLevel) {
        self.levels[rule] = Some(level);
    }

    /// Every rule with a setting.
    pub fn rules(&self) -> impl Iterator<Item = RuleId> {
        RuleId::ALL.into_iter().filter(|&rule| self.levels[rule].is_some())
    }

    pub(crate) const fn options(&self) -> &OptionSettings {
        &self.options
    }
}

/// Each rule by name, as `luck.json` writes it.
#[cfg(feature = "serde")]
impl Serialize for RuleSettings {
    fn serialize<Target>(&self, serializer: Target) -> Result<Target::Ok, Target::Error>
    where
        Target: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.rules().count()))?;
        for rule in RuleId::ALL {
            if let Some(level) = self.levels[rule] {
                self.options.serialize_entry(rule, level, &mut map)?;
            }
        }
        map.end()
    }
}

// Written by hand to read each entry straight from the source, so a format
// that tracks positions, as `luck.json`'s does, places an error on the entry
// at fault, where a derive over a map would place it on the whole map.
#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for RuleSettings {
    fn deserialize<Source>(deserializer: Source) -> Result<Self, Source::Error>
    where
        Source: Deserializer<'de>,
    {
        deserializer.deserialize_map(RuleSettingsVisitor)
    }
}

#[cfg(feature = "serde")]
struct RuleSettingsVisitor;

#[cfg(feature = "serde")]
impl<'de> Visitor<'de> for RuleSettingsVisitor {
    type Value = RuleSettings;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a setting per rule name")
    }

    fn visit_map<Access>(self, mut map: Access) -> Result<Self::Value, Access::Error>
    where
        Access: MapAccess<'de>,
    {
        let mut settings = RuleSettings::default();
        while let Some(rule) = map.next_key::<RuleId>()? {
            let level = settings.options.next_setting(rule, &mut map)?;
            if settings.levels[rule].replace(level).is_some() {
                return Err(Error::custom(format_args!("duplicate rule `{}`", rule.name())));
            }
        }
        Ok(settings)
    }
}

/// A rule's entry in `rules`, which keeps `options` for one that takes them,
/// and returns its level.
#[cfg(feature = "serde")]
pub(crate) fn next_setting<'de, Options, Access>(
    rule: RuleId,
    options: &mut Option<Options>,
    map: &mut Access,
) -> Result<RuleLevel, Access::Error>
where
    Options: RuleOptions + Deserialize<'de>,
    Access: MapAccess<'de>,
{
    let setting = map.next_value_seed(SettingSeed(rule, PhantomData))?;
    Ok(match setting {
        RuleSetting::Level(level) => level,
        RuleSetting::WithOptions(level, given) => {
            *options = Some(given);
            level
        }
    })
}

#[cfg(feature = "serde")]
pub(crate) fn serialize_entry<Options, Map>(
    rule: RuleId,
    level: RuleLevel,
    options: Option<&Options>,
    map: &mut Map,
) -> Result<(), Map::Error>
where
    Options: Serialize,
    Map: SerializeMap,
{
    let setting =
        options.map_or(RuleSetting::Level(level), |given| RuleSetting::WithOptions(level, given));
    map.serialize_entry(rule.name(), &setting)
}

/// One rule's entry in `rules`.
#[cfg(feature = "serde")]
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(rename = "RuleSetting_{Options}"))]
#[serde(untagged)]
pub(crate) enum RuleSetting<Options> {
    /// `"off"`, `"warn"`, or `"error"`.
    Level(RuleLevel),
    /// A level and the rule's options: `[level, options]`.
    WithOptions(RuleLevel, Options),
}

/// The entry of the rule it names, which takes options only when it has some.
///
/// Written by hand because an untagged derive reports every mistake as "data
/// did not match any variant", without the bad level or option key.
#[cfg(feature = "serde")]
struct SettingSeed<Options>(RuleId, PhantomData<Options>);

#[cfg(feature = "serde")]
impl<'de, Options> DeserializeSeed<'de> for SettingSeed<Options>
where
    Options: RuleOptions + Deserialize<'de>,
{
    type Value = RuleSetting<Options>;

    fn deserialize<Source>(self, deserializer: Source) -> Result<Self::Value, Source::Error>
    where
        Source: Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

#[cfg(feature = "serde")]
impl<'de, Options> Visitor<'de> for SettingSeed<Options>
where
    Options: RuleOptions + Deserialize<'de>,
{
    type Value = RuleSetting<Options>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(if Options::IS_NONE {
            "a level"
        } else {
            "a level or `[level, options]`"
        })
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
        if Options::IS_NONE {
            return Err(Error::custom(format_args!(
                "`{}` takes no options, so set only its level",
                self.0.name()
            )));
        }
        let level = seq.next_element()?.ok_or_else(|| Error::invalid_length(0, &self))?;
        let options = seq.next_element()?.ok_or_else(|| Error::invalid_length(1, &self))?;
        if seq.next_element::<IgnoredAny>()?.is_some() {
            return Err(Error::invalid_length(3, &self));
        }
        Ok(RuleSetting::WithOptions(level, options))
    }
}

/// Every rule a property, so an editor completes the names and flags one no
/// rule has.
#[cfg(feature = "schemars")]
impl JsonSchema for RuleSettings {
    fn schema_name() -> Cow<'static, str> {
        "RuleSettings".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let properties: BTreeMap<&str, Schema> = RuleId::ALL
            .into_iter()
            .map(|rule| {
                let mut schema = OptionSettings::setting_schema(rule, generator);
                schema.insert("description".to_owned(), rule.description().into());
                (rule.name(), schema)
            })
            .collect();
        json_schema!({
            "description": "A setting per rule, keyed by rule name.",
            "type": "object",
            "properties": properties,
            "additionalProperties": false,
        })
    }
}

/// The schema of an entry in `rules` that takes `Options`.
#[cfg(feature = "schemars")]
pub(crate) fn setting_schema<Options>(generator: &mut SchemaGenerator) -> Schema
where
    Options: RuleOptions + JsonSchema,
{
    if Options::IS_NONE {
        generator.subschema_for::<RuleLevel>()
    } else {
        generator.subschema_for::<RuleSetting<Options>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint_config(lint_json: &str) -> LintConfig {
        serde_json::from_str(lint_json).expect("deserialize")
    }

    fn lint_error(lint_json: &str) -> String {
        serde_json::from_str::<LintConfig>(lint_json).unwrap_err().to_string()
    }

    /// The JSON of `lint`'s `rules`.
    fn rules_json(lint: &LintConfig) -> String {
        serde_json::to_string(&lint.rules).expect("serialize")
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
        assert_eq!(lint.rules.level(RuleId::UnusedVariable), Some(RuleLevel::Off));
        assert_eq!(lint.rules.level(RuleId::UndefinedVariable), Some(RuleLevel::Error));
        assert_eq!(
            rules_json(&lint),
            r#"{"undefined_variable":"error","unused_variable":"off","restricted_module_paths":["error",{"paths":["secret.path"]}],"cyclomatic_complexity":["warn",{"max":15}]}"#
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
            r#"{"categories":{"performance":"warn"},"rules":{"unused_variable":"off","restricted_module_paths":"error","cyclomatic_complexity":["warn",{"max":15}]},"extra_globals":["vim"]}"#
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
        assert_eq!(lint.rules.level(RuleId::CyclomaticComplexity), Some(RuleLevel::Warn));
        assert_eq!(rules_json(&lint), r#"{"cyclomatic_complexity":"warn"}"#);
    }

    #[test]
    fn merge_onto_concatenates_lists_base_first_without_repeats() {
        let base =
            lint_config(r#"{ "extra_globals": ["a", "b"], "rules": { "shadowing": "off" } }"#);
        let child =
            lint_config(r#"{ "extra_globals": ["b", "c"], "rules": { "empty_block": "error" } }"#);
        let merged = child.merge_onto(base);
        assert_eq!(merged.extra_globals, ["a", "b", "c"]);
        assert_eq!(
            merged.rules.rules().collect::<Vec<_>>(),
            [RuleId::EmptyBlock, RuleId::Shadowing]
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
        assert_eq!(
            rules_json(&lint),
            r#"{"unused_variable":"error","empty_block":"warn","cyclomatic_complexity":["off",{"max":10}]}"#
        );
    }

    /// A child that only raises a rule's severity mustn't silence it by
    /// dropping the base's options.
    #[test]
    fn merge_onto_keeps_the_base_options_and_merges_the_child_s() {
        let base = r#"{
            "rules": {
                "cyclomatic_complexity": ["warn", { "max": 10 }],
                "restricted_module_paths": ["warn", { "paths": ["secret.*"] }]
            }
        }"#;
        let raised = lint_config(r#"{ "rules": { "cyclomatic_complexity": "error" } }"#)
            .merge_onto(lint_config(base));
        assert_eq!(
            rules_json(&raised),
            r#"{"restricted_module_paths":["warn",{"paths":["secret.*"]}],"cyclomatic_complexity":["error",{"max":10}]}"#
        );

        let extended = lint_config(
            r#"{
                "rules": {
                    "cyclomatic_complexity": ["warn", { "max": 4 }],
                    "restricted_module_paths": ["error", { "paths": ["legacy", "secret.*"] }]
                }
            }"#,
        )
        .merge_onto(lint_config(base));
        assert_eq!(
            rules_json(&extended),
            r#"{"restricted_module_paths":["error",{"paths":["secret.*","legacy"]}],"cyclomatic_complexity":["warn",{"max":4}]}"#
        );
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
        assert!(error.contains("`unused_variable` takes no options"), "{error}");
    }

    #[test]
    fn rejects_a_name_no_rule_has_and_suggests_a_close_one() {
        let typo = lint_error(r#"{ "rules": { "unsued_variable": "warn" } }"#);
        assert!(
            typo.contains("unknown lint rule `unsued_variable`, did you mean `unused_variable`?"),
            "{typo}"
        );
        let parse_error_code = lint_error(r#"{ "rules": { "E008": "off" } }"#);
        assert!(parse_error_code.contains("unknown lint rule `E008`"), "{parse_error_code}");
    }

    #[test]
    fn rejects_a_rule_set_twice() {
        for rule in ["unused_variable", "cyclomatic_complexity"] {
            let error =
                lint_error(&format!(r#"{{ "rules": {{ "{rule}": "warn", "{rule}": "off" }} }}"#));
            assert!(error.contains(&format!("duplicate rule `{rule}`")), "{error}");
        }
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
        let mut lint =
            lint_config(r#"{ "rules": { "cyclomatic_complexity": ["warn", { "max": 4 }] } }"#);
        lint.rules.set_level(RuleId::CyclomaticComplexity, RuleLevel::Error);
        lint.rules.set_level(RuleId::RestrictedModulePaths, RuleLevel::Off);
        lint.rules.set_level(RuleId::UnusedVariable, RuleLevel::Warn);
        assert_eq!(
            rules_json(&lint),
            r#"{"unused_variable":"warn","restricted_module_paths":"off","cyclomatic_complexity":["error",{"max":4}]}"#
        );
    }

    #[test]
    fn a_rule_s_own_level_wins_then_its_category_s_then_its_default() {
        let mut lint = LintConfig::default();
        assert_eq!(lint.severity(RuleId::UndefinedVariable), Some(Severity::Error));
        assert_eq!(lint.severity(RuleId::EmptyBlock), None, "only correctness runs by default");
        lint.categories.set_level(Category::Correctness, RuleLevel::Warn);
        assert_eq!(lint.severity(RuleId::UndefinedVariable), Some(Severity::Warning));
        lint.rules.set_level(RuleId::UndefinedVariable, RuleLevel::Off);
        assert_eq!(lint.severity(RuleId::UndefinedVariable), None);
    }
}
