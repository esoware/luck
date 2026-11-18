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

use crate::rule::{NodeRule, Rule, WholeRule};
use crate::suggest::closest;

#[derive(Clone, Copy)]
pub(crate) enum RuleEntry {
    Whole(&'static dyn WholeRule),
    Node(&'static dyn NodeRule),
    /// It reads which `allow`s the other rules' diagnostics met, so it runs
    /// once they all have.
    UnusedLintFilter(&'static unused_lint_filter::UnusedLintFilter),
}

impl RuleEntry {
    pub(crate) fn rule(&self) -> &'static dyn Rule {
        match *self {
            Self::Whole(rule) => rule,
            Self::Node(rule) => rule,
            Self::UnusedLintFilter(rule) => rule,
        }
    }
}

/// Every rule. Diagnostics that start at the same offset come out in this
/// order.
///
/// Rules are stateless and read their settings from `LintContext::config`,
/// so the registry can be a static.
pub(crate) static RULES: &[RuleEntry] = &[
    RuleEntry::Whole(&undefined_variable::UndefinedVariable),
    RuleEntry::Whole(&unused_variable::UnusedVariable),
    RuleEntry::Whole(&setting_global::SettingGlobal),
    RuleEntry::Node(&duplicate_keys::DuplicateKeys),
    RuleEntry::Node(&compare_nan::CompareNan),
    RuleEntry::Node(&constant_table_comparison::ConstantTableComparison),
    RuleEntry::Whole(&almost_swapped::AlmostSwapped),
    RuleEntry::Node(&type_check_inside_call::TypeCheckInsideCall),
    RuleEntry::Node(&incorrect_stdlib_use::IncorrectStdlibUse),
    RuleEntry::Node(&deprecated::Deprecated),
    RuleEntry::Node(&duplicate_conditions::DuplicateConditions),
    RuleEntry::Whole(&invalid_lint_filter::InvalidLintFilter),
    RuleEntry::Node(&for_range::ForRange),
    RuleEntry::Node(&bad_string_escape::BadStringEscape),
    RuleEntry::Node(&comparison_precedence::ComparisonPrecedence),
    RuleEntry::Node(&integer_parsing::IntegerParsing),
    RuleEntry::Node(&divide_by_zero::DivideByZero),
    RuleEntry::Node(&misleading_and_or::MisleadingAndOr),
    RuleEntry::Node(&implicit_return::ImplicitReturn),
    RuleEntry::Node(&format_string::FormatString),
    RuleEntry::Whole(&value_overwritten_before_read::ValueOverwrittenBeforeRead),
    RuleEntry::Whole(&accessing_uninitialized::AccessingUninitialized),
    RuleEntry::Whole(&mutating_uninitialized::MutatingUninitialized),
    RuleEntry::Node(&restricted_module_paths::RestrictedModulePaths),
    RuleEntry::Node(&empty_block::EmptyBlock),
    RuleEntry::Node(&reversed_for_loop::ReversedForLoop),
    RuleEntry::Node(&unbalanced_assignment::UnbalancedAssignment),
    RuleEntry::Node(&if_same_then_else::IfSameThenElse),
    RuleEntry::Whole(&unreachable_code::UnreachableCode),
    RuleEntry::Node(&must_use::MustUse),
    RuleEntry::Node(&placeholder_read::PlaceholderRead),
    RuleEntry::Node(&mixed_table::MixedTable),
    RuleEntry::Whole(&loop_executes_once::LoopExecutesOnce),
    RuleEntry::Whole(&shadowing::Shadowing),
    RuleEntry::Node(&parenthesized_conditions::ParenthesizedConditions),
    RuleEntry::Node(&redundant_nil_init::RedundantNilInit),
    RuleEntry::Node(&string_index_to_field::StringIndexToField),
    RuleEntry::Whole(&merge_adjacent_locals::MergeAdjacentLocals),
    RuleEntry::Whole(&unused_label::UnusedLabel),
    RuleEntry::Whole(&unused_argument::UnusedArgument),
    RuleEntry::Whole(&unused_loop_variable::UnusedLoopVariable),
    RuleEntry::Whole(&set_but_never_read::SetButNeverRead),
    RuleEntry::Whole(&redefining_local::RedefiningLocal),
    RuleEntry::Whole(&multiple_statements_per_line::MultipleStatementsPerLine),
    RuleEntry::Node(&cyclomatic_complexity::CyclomaticComplexity),
    RuleEntry::Whole(&global_usage::GlobalUsage),
    RuleEntry::Whole(&no_implicit_globals::NoImplicitGlobals),
    RuleEntry::Whole(&manual_table_clone::ManualTableClone),
    RuleEntry::Whole(&mismatched_arg_count::MismatchedArgCount),
    RuleEntry::Whole(&global_used_as_local::GlobalUsedAsLocal),
    RuleEntry::Whole(&builtin_global_write::BuiltinGlobalWrite),
    RuleEntry::Whole(&duplicate_function::DuplicateFunction),
    RuleEntry::Whole(&redundant_return::RedundantReturn),
    RuleEntry::Whole(&comment_directive::CommentDirective),
    RuleEntry::Node(&redundant_native_attribute::RedundantNativeAttribute),
    RuleEntry::Node(&unnecessary_negation::UnnecessaryNegation),
    RuleEntry::Node(&duplicate_parameter::DuplicateParameter),
    RuleEntry::Node(&table_operations::TableOperations),
    RuleEntry::Node(&unnecessary_assert::UnnecessaryAssert),
    RuleEntry::Node(&ambiguous_newline_call::AmbiguousNewlineCall),
    RuleEntry::Node(&unknown_type::UnknownType),
    RuleEntry::Node(&roblox_incorrect_color3_new_bounds::RobloxIncorrectColor3NewBounds),
    RuleEntry::Node(&roblox_suspicious_udim2_new::RobloxSuspiciousUdim2New),
    RuleEntry::Node(&roblox_manual_fromscale_or_fromoffset::RobloxManualFromScaleOrFromOffset),
    RuleEntry::Node(&roblox_unknown_enum_member::RobloxUnknownEnumMember),
    RuleEntry::UnusedLintFilter(&unused_lint_filter::UnusedLintFilter),
];

pub fn rules() -> impl Iterator<Item = &'static dyn Rule> {
    RULES.iter().map(RuleEntry::rule)
}

/// The rule named `name`, which `luck.json` and the `-A`, `-W`, and `-D`
/// flags look rules up by.
///
/// # Errors
///
/// Returns the message for a name no rule has, which suggests a rule whose
/// name is close.
pub fn rule_named(name: &str) -> Result<&'static dyn Rule, String> {
    rules().find(|rule| rule.name() == name).ok_or_else(|| {
        closest_rule_name(name, rules().map(Rule::name)).map_or_else(
            || format!("unknown lint rule `{name}`"),
            |suggestion| format!("unknown lint rule `{name}`, did you mean `{suggestion}`?"),
        )
    })
}

/// A far-off "did you mean" is worse than none.
pub(crate) fn closest_rule_name(
    needle: &str,
    candidates: impl IntoIterator<Item = &'static str>,
) -> Option<&'static str> {
    // Up to a third of the longer name, and never less than 2.
    closest(needle, candidates, |distance, candidate| {
        distance <= 2 || distance.saturating_mul(3) <= needle.len().max(candidate.len())
    })
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashSet;

    use super::*;

    #[test]
    fn closest_rule_name_picks_a_near_neighbor() {
        assert_eq!(
            closest_rule_name("unsued_variable", ["unused_variable", "shadowing"]),
            Some("unused_variable")
        );
    }

    #[test]
    fn closest_rule_name_is_none_for_far_strings() {
        assert!(closest_rule_name("xx", ["unused_variable"]).is_none());
    }

    #[test]
    fn rule_named_finds_a_rule_and_suggests_for_a_typo() {
        assert_eq!(rule_named("shadowing").map(Rule::name), Ok("shadowing"));
        assert_eq!(
            rule_named("unsued_variable").map(Rule::name),
            Err("unknown lint rule `unsued_variable`, did you mean `unused_variable`?".to_owned())
        );
        assert_eq!(
            rule_named("parse_error").map(Rule::name),
            Err("unknown lint rule `parse_error`".to_owned()),
            "config can't turn a parse error off, so it doesn't name one"
        );
    }

    #[test]
    fn rule_count_locked() {
        // If this changes, update the rule count in the root README.md.
        assert_eq!(RULES.len(), 66);
    }

    #[test]
    fn rule_names_are_unique() {
        let mut seen_names = FxHashSet::default();
        for name in rules().map(Rule::name) {
            assert!(seen_names.insert(name), "duplicate rule name `{name}`");
        }
    }

    #[test]
    fn unused_lint_filter_comes_last_since_it_runs_last() {
        assert!(matches!(RULES.last(), Some(RuleEntry::UnusedLintFilter(_))));
    }
}
