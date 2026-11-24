mod accessing_uninitialized;
mod almost_swapped;
mod ambiguous_newline_call;
mod bad_string_escape;
mod builtin_global_write;
mod comment_directive;
mod compare_nan;
mod comparison_precedence;
mod constant_table_comparison;
mod cyclomatic_complexity;
mod deprecated;
mod divide_by_zero;
mod duplicate_conditions;
mod duplicate_function;
mod duplicate_keys;
mod duplicate_parameter;
mod empty_block;
mod for_range;
mod format_string;
mod global_usage;
mod global_used_as_local;
mod if_same_then_else;
mod implicit_return;
mod incorrect_stdlib_use;
mod integer_parsing;
mod invalid_lint_filter;
mod loop_executes_once;
mod manual_table_clone;
mod merge_adjacent_locals;
mod misleading_and_or;
mod mismatched_arg_count;
mod mixed_table;
mod multiple_statements_per_line;
mod must_use;
mod mutating_uninitialized;
mod no_implicit_globals;
mod parenthesized_conditions;
mod placeholder_read;
mod redefining_local;
mod redundant_native_attribute;
mod redundant_nil_init;
mod redundant_return;
mod restricted_module_paths;
mod reversed_for_loop;
mod roblox_incorrect_color3_new_bounds;
mod roblox_manual_fromscale_or_fromoffset;
mod roblox_suspicious_udim2_new;
mod roblox_unknown_enum_member;
mod set_but_never_read;
mod setting_global;
mod shadowing;
mod string_index_to_field;
mod table_operations;
mod type_check_inside_call;
mod unbalanced_assignment;
mod undefined_variable;
mod unknown_type;
mod unnecessary_assert;
mod unnecessary_negation;
mod unreachable_code;
mod unused_argument;
mod unused_label;
mod unused_lint_filter;
mod unused_loop_variable;
mod unused_variable;
mod value_overwritten_before_read;

#[cfg(feature = "serde")]
use std::fmt;
use std::ops::{Index, IndexMut};
use std::str::FromStr;

use luck_diagnostics::Severity;
use luck_semantic::{AstNode, AstTypesBitset};
#[cfg(feature = "schemars")]
use schemars::{Schema, SchemaGenerator};
#[cfg(feature = "serde")]
use serde::de::{Deserialize, Deserializer, Error, MapAccess, Visitor};
#[cfg(feature = "serde")]
use serde::ser::SerializeMap;

use crate::config::merge_options;
#[cfg(feature = "schemars")]
use crate::config::setting_schema;
#[cfg(feature = "serde")]
use crate::config::{RuleLevel, next_setting, serialize_entry};
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Category;
use crate::fix::FixKind;
use crate::rule::{Rule, RuleMeta};
use crate::suggest::closest;

/// Generates what reads every rule by id, from each rule's module, which is
/// named after the rule: [`RuleId`], `RuleEnum`, and `OptionSettings`.
macro_rules! declare_all_lint_rules {
    ($($module:ident::$rule:ident),+ $(,)?) => {
        /// A lint rule, which config, directives, and diagnostics name by its
        /// [`name`](Self::name), as oxlint's `RuleEnum` numbers its rules.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum RuleId {
            $($rule,)+
        }

        impl RuleId {
            /// Every rule. Diagnostics that start at the same offset come out
            /// in this order.
            pub const ALL: [Self; Self::COUNT] = [$(Self::$rule,)+];

            pub const COUNT: usize = [$(Self::$rule),+].len();

            /// What `luck.json`, the `-A`, `-W`, and `-D` flags, directives,
            /// and diagnostics call the rule, which is its module's name.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$rule => stringify!($module),)+
                }
            }

            pub const fn category(self) -> Category {
                match self {
                    $(Self::$rule => <$module::$rule as RuleMeta>::CATEGORY,)+
                }
            }

            /// What the rule reports at while config sets no level for it
            /// or its category, which only a correctness rule runs under.
            pub const fn default_severity(self) -> Severity {
                match self {
                    $(Self::$rule => <$module::$rule as RuleMeta>::DEFAULT_SEVERITY,)+
                }
            }

            pub const fn description(self) -> &'static str {
                match self {
                    $(Self::$rule => <$module::$rule as RuleMeta>::DESCRIPTION,)+
                }
            }

            /// The kinds the rule's fixes come in, empty when it offers none.
            /// Some of its diagnostics may still have no fix.
            pub const fn fix_kind(self) -> FixKind {
                match self {
                    $(Self::$rule => <$module::$rule as RuleMeta>::FIX,)+
                }
            }

            /// The rule's source on GitHub, at the release this linter is,
            /// where its doc comment and its test cases document it.
            pub const fn docs_url(self) -> &'static str {
                match self {
                    $(Self::$rule => concat!(
                        env!("CARGO_PKG_REPOSITORY"),
                        "/blob/v",
                        env!("CARGO_PKG_VERSION"),
                        "/crates/luck_linter/src/rules/",
                        stringify!($module),
                        ".rs",
                    ),)+
                }
            }

            /// The rule called `name`, if any is.
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $(stringify!($module) => Some(Self::$rule),)+
                    _ => None,
                }
            }
        }

        /// Each rule as its options configure it, as oxlint's `RuleEnum` is.
        #[derive(Debug)]
        pub(crate) enum RuleEnum {
            $($rule($module::$rule),)+
        }

        impl RuleEnum {
            pub(crate) const fn id(&self) -> RuleId {
                match self {
                    $(Self::$rule(_) => RuleId::$rule,)+
                }
            }

            pub(crate) const fn node_types(&self) -> AstTypesBitset {
                match self {
                    $(Self::$rule(_) => <$module::$rule as Rule>::NODE_TYPES,)+
                }
            }

            pub(crate) fn should_run(&self, host: &ContextHost<'_>) -> bool {
                match self {
                    $(Self::$rule(rule) => rule.should_run(host),)+
                }
            }

            pub(crate) fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
                match self {
                    $(Self::$rule(rule) => rule.run(node, context),)+
                }
            }

            pub(crate) fn run_once(&self, context: &LintContext<'_>) {
                match self {
                    $(Self::$rule(rule) => rule.run_once(context),)+
                }
            }
        }

        /// The options `[level, options]` gives each rule.
        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        pub(crate) struct OptionSettings {
            $($module: Option<<$module::$rule as RuleMeta>::Options>,)+
        }

        impl OptionSettings {
            pub(crate) fn merge_onto(self, base: Self) -> Self {
                Self {
                    $($module: merge_options(self.$module, base.$module),)+
                }
            }

            /// Every rule, built from its options.
            pub(crate) fn configured(&self) -> RuleTable<RuleEnum> {
                RuleTable([$(RuleEnum::$rule(self.$module.clone().map_or_else(
                    Default::default,
                    <$module::$rule as RuleMeta>::from_options,
                )),)+])
            }

            /// Reads `rule`'s setting from `map`, keeps its options, and
            /// returns its level.
            #[cfg(feature = "serde")]
            pub(crate) fn next_setting<'de, Access>(
                &mut self,
                rule: RuleId,
                map: &mut Access,
            ) -> Result<RuleLevel, Access::Error>
            where
                Access: MapAccess<'de>,
            {
                match rule {
                    $(RuleId::$rule => next_setting(rule, &mut self.$module, map),)+
                }
            }

            #[cfg(feature = "serde")]
            pub(crate) fn serialize_entry<Map>(
                &self,
                rule: RuleId,
                level: RuleLevel,
                map: &mut Map,
            ) -> Result<(), Map::Error>
            where
                Map: SerializeMap,
            {
                match rule {
                    $(RuleId::$rule => serialize_entry(rule, level, self.$module.as_ref(), map),)+
                }
            }

            #[cfg(feature = "schemars")]
            pub(crate) fn setting_schema(rule: RuleId, generator: &mut SchemaGenerator) -> Schema {
                match rule {
                    $(RuleId::$rule => {
                        setting_schema::<<$module::$rule as RuleMeta>::Options>(generator)
                    })+
                }
            }
        }
    };
}

declare_all_lint_rules! {
    undefined_variable::UndefinedVariable,
    unused_variable::UnusedVariable,
    setting_global::SettingGlobal,
    duplicate_keys::DuplicateKeys,
    compare_nan::CompareNan,
    constant_table_comparison::ConstantTableComparison,
    almost_swapped::AlmostSwapped,
    type_check_inside_call::TypeCheckInsideCall,
    incorrect_stdlib_use::IncorrectStdlibUse,
    deprecated::Deprecated,
    duplicate_conditions::DuplicateConditions,
    invalid_lint_filter::InvalidLintFilter,
    for_range::ForRange,
    bad_string_escape::BadStringEscape,
    comparison_precedence::ComparisonPrecedence,
    integer_parsing::IntegerParsing,
    divide_by_zero::DivideByZero,
    misleading_and_or::MisleadingAndOr,
    implicit_return::ImplicitReturn,
    format_string::FormatString,
    value_overwritten_before_read::ValueOverwrittenBeforeRead,
    accessing_uninitialized::AccessingUninitialized,
    mutating_uninitialized::MutatingUninitialized,
    restricted_module_paths::RestrictedModulePaths,
    empty_block::EmptyBlock,
    reversed_for_loop::ReversedForLoop,
    unbalanced_assignment::UnbalancedAssignment,
    if_same_then_else::IfSameThenElse,
    unreachable_code::UnreachableCode,
    must_use::MustUse,
    placeholder_read::PlaceholderRead,
    mixed_table::MixedTable,
    loop_executes_once::LoopExecutesOnce,
    shadowing::Shadowing,
    parenthesized_conditions::ParenthesizedConditions,
    redundant_nil_init::RedundantNilInit,
    string_index_to_field::StringIndexToField,
    merge_adjacent_locals::MergeAdjacentLocals,
    unused_label::UnusedLabel,
    unused_argument::UnusedArgument,
    unused_loop_variable::UnusedLoopVariable,
    set_but_never_read::SetButNeverRead,
    redefining_local::RedefiningLocal,
    multiple_statements_per_line::MultipleStatementsPerLine,
    cyclomatic_complexity::CyclomaticComplexity,
    global_usage::GlobalUsage,
    no_implicit_globals::NoImplicitGlobals,
    manual_table_clone::ManualTableClone,
    mismatched_arg_count::MismatchedArgCount,
    global_used_as_local::GlobalUsedAsLocal,
    builtin_global_write::BuiltinGlobalWrite,
    duplicate_function::DuplicateFunction,
    redundant_return::RedundantReturn,
    comment_directive::CommentDirective,
    redundant_native_attribute::RedundantNativeAttribute,
    unnecessary_negation::UnnecessaryNegation,
    duplicate_parameter::DuplicateParameter,
    table_operations::TableOperations,
    unnecessary_assert::UnnecessaryAssert,
    ambiguous_newline_call::AmbiguousNewlineCall,
    unknown_type::UnknownType,
    roblox_incorrect_color3_new_bounds::RobloxIncorrectColor3NewBounds,
    roblox_suspicious_udim2_new::RobloxSuspiciousUdim2New,
    roblox_manual_fromscale_or_fromoffset::RobloxManualFromScaleOrFromOffset,
    roblox_unknown_enum_member::RobloxUnknownEnumMember,
    unused_lint_filter::UnusedLintFilter,
}

pub(crate) use unused_lint_filter::UnusedLintFilter;

impl RuleId {
    const fn index(self) -> usize {
        self as usize
    }
}

impl FromStr for RuleId {
    type Err = String;

    /// # Errors
    ///
    /// Returns the message for a name no rule has, which suggests a rule
    /// whose name is close.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::from_name(name).ok_or_else(|| {
            closest_rule_name(name).map_or_else(
                || format!("unknown lint rule `{name}`"),
                |rule| format!("unknown lint rule `{name}`, did you mean `{}`?", rule.name()),
            )
        })
    }
}

/// A key of `rules`, which a format that tracks positions places an unknown
/// name's error on.
#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for RuleId {
    fn deserialize<Source>(deserializer: Source) -> Result<Self, Source::Error>
    where
        Source: Deserializer<'de>,
    {
        deserializer.deserialize_str(RuleIdVisitor)
    }
}

#[cfg(feature = "serde")]
struct RuleIdVisitor;

#[cfg(feature = "serde")]
impl Visitor<'_> for RuleIdVisitor {
    type Value = RuleId;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a rule name")
    }

    fn visit_str<Failure>(self, v: &str) -> Result<Self::Value, Failure>
    where
        Failure: Error,
    {
        v.parse().map_err(Failure::custom)
    }
}

/// The rule whose name is closest to `needle`, when one is close enough,
/// since a far-off "did you mean" is worse than none.
pub(crate) fn closest_rule_name(needle: &str) -> Option<RuleId> {
    let name = closest(needle, RuleId::ALL.map(RuleId::name), |distance, candidate| {
        // Up to a third of the longer name, and never less than 2.
        distance <= 2 || distance * 3 <= needle.len().max(candidate.len())
    })?;
    RuleId::from_name(name)
}

/// A value per rule, which an id indexes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleTable<T>([T; RuleId::COUNT]);

impl<T> RuleTable<T> {
    pub(crate) fn from_fn<Value>(value: Value) -> Self
    where
        Value: FnMut(RuleId) -> T,
    {
        Self(RuleId::ALL.map(value))
    }
}

impl<T> Default for RuleTable<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::from_fn(|_| T::default())
    }
}

impl<T> Index<RuleId> for RuleTable<T> {
    type Output = T;

    fn index(&self, index: RuleId) -> &T {
        &self.0[index.index()]
    }
}

impl<T> IndexMut<RuleId> for RuleTable<T> {
    fn index_mut(&mut self, index: RuleId) -> &mut T {
        &mut self.0[index.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_rule_name_picks_a_near_neighbor_and_no_far_one() {
        assert_eq!(closest_rule_name("unsued_variable"), Some(RuleId::UnusedVariable));
        assert_eq!(closest_rule_name("xx"), None);
    }

    #[test]
    fn a_name_parses_to_its_rule_and_a_typo_suggests_one() {
        for rule in RuleId::ALL {
            assert_eq!(rule.name().parse(), Ok(rule));
        }
        assert_eq!(
            "unsued_variable".parse::<RuleId>(),
            Err("unknown lint rule `unsued_variable`, did you mean `unused_variable`?".to_owned())
        );
        assert_eq!(
            "E008".parse::<RuleId>(),
            Err("unknown lint rule `E008`".to_owned()),
            "config can't turn a parse error off, so it doesn't name one"
        );
    }

    #[test]
    fn rule_count_locked() {
        // If this changes, update the rule count in the root README.md.
        assert_eq!(RuleId::COUNT, 66);
    }

    #[test]
    fn each_rule_links_to_its_own_source_file() {
        for rule in RuleId::ALL {
            assert_eq!(
                rule.docs_url(),
                format!(
                    "https://github.com/esoware/luck/blob/v{}/crates/luck_linter/src/rules/{}.rs",
                    env!("CARGO_PKG_VERSION"),
                    rule.name()
                )
            );
        }
    }
}
