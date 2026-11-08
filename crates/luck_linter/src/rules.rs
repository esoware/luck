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
mod unused_loop_variable;
mod unused_variable;
mod value_overwritten_before_read;

use core::iter;

use luck_core::LintConfig;

use crate::rule::{NodeRule, Rule, WholeRule};

/// How a rule takes part in the lint pass.
#[derive(Clone, Copy)]
pub(crate) enum RuleEntry {
    /// Walks the tree itself in `WholeRule::check`.
    Whole(&'static dyn WholeRule),
    /// Driven by the bus's shared walk.
    Node(&'static dyn NodeRule),
}

impl RuleEntry {
    pub(crate) fn rule(&self) -> &'static dyn Rule {
        match *self {
            Self::Whole(rule) => rule,
            Self::Node(rule) => rule,
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
    RuleEntry::Whole(&bad_string_escape::BadStringEscape),
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
    RuleEntry::Node(&loop_executes_once::LoopExecutesOnce),
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
];

/// Rule name on parse-error diagnostics. It isn't a `Rule`, but it's
/// registered so directives and config that name it aren't flagged as
/// unknown.
pub(crate) const PARSE_ERROR_PSEUDO_RULE: &str = "parse_error";

/// Every rule name in registration order, then `parse_error`.
///
/// `invalid_lint_filter` and `unknown_rule_names` check directive and config
/// names against this list.
pub(crate) fn registered_rule_names() -> impl Iterator<Item = &'static str> {
    RULES.iter().map(|entry| entry.rule().name()).chain(iter::once(PARSE_ERROR_PSEUDO_RULE))
}

pub(crate) fn is_registered_rule_name(name: &str) -> bool {
    registered_rule_names().any(|registered| registered == name)
}

/// Every registered rule, in registration order.
pub fn rules() -> impl Iterator<Item = &'static dyn Rule> {
    RULES.iter().map(RuleEntry::rule)
}

/// Returns the config's rule-override names that aren't registered rules.
///
/// The names come back sorted. They're checked against the same list
/// `invalid_lint_filter` uses, which includes `parse_error`.
#[must_use]
pub fn unknown_rule_names(config: &LintConfig) -> Vec<String> {
    let mut unknown_names: Vec<String> = config
        .rule_overrides
        .keys()
        .filter(|name| !is_registered_rule_name(name))
        .cloned()
        .collect();
    unknown_names.sort();
    unknown_names
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashSet;

    use super::*;

    #[test]
    fn registered_names_are_every_rule_then_parse_error() {
        let mut expected_names: Vec<&str> = RULES.iter().map(|entry| entry.rule().name()).collect();
        expected_names.push(PARSE_ERROR_PSEUDO_RULE);
        assert_eq!(registered_rule_names().collect::<Vec<_>>(), expected_names);
    }

    #[test]
    fn rule_count_locked() {
        // If this changes, update the rule count in the root README.md.
        assert_eq!(RULES.len(), 65);
    }

    #[test]
    fn registered_names_unique() {
        let mut seen_names = FxHashSet::default();
        for name in registered_rule_names() {
            assert!(seen_names.insert(name), "duplicate name `{name}` in registered list");
        }
    }
}
