pub mod accessing_uninitialized;
pub mod almost_swapped;
pub mod ambiguous_newline_call;
pub mod bad_string_escape;
pub mod builtin_global_write;
pub mod comment_directive;
pub mod compare_nan;
pub mod comparison_precedence;
pub mod constant_table_comparison;
pub mod cyclomatic_complexity;
pub mod deprecated;
pub mod divide_by_zero;
pub mod duplicate_conditions;
pub mod duplicate_function;
pub mod duplicate_keys;
pub mod duplicate_parameter;
pub mod empty_block;
pub mod for_range;
pub mod format_string;
pub mod global_usage;
pub mod global_used_as_local;
pub mod if_same_then_else;
pub mod implicit_return;
pub mod incorrect_stdlib_use;
pub mod integer_parsing;
pub mod invalid_lint_filter;
pub mod loop_executes_once;
pub mod manual_table_clone;
pub mod merge_adjacent_locals;
pub mod misleading_and_or;
pub mod mismatched_arg_count;
pub mod mixed_table;
pub mod multiple_statements_per_line;
pub mod must_use;
pub mod mutating_uninitialized;
pub mod parenthesized_conditions;
pub mod placeholder_read;
pub mod redefining_local;
pub mod redundant_native_attribute;
pub mod redundant_nil_init;
pub mod redundant_return;
pub mod restricted_module_paths;
pub mod reversed_for_loop;
pub mod roblox_incorrect_color3_new_bounds;
pub mod roblox_manual_fromscale_or_fromoffset;
pub mod roblox_suspicious_udim2_new;
pub mod set_but_never_read;
pub mod setting_global;
pub mod shadowing;
pub mod string_index_to_field;
pub mod table_operations;
pub mod type_check_inside_call;
pub mod unbalanced_assignment;
pub mod undefined_variable;
pub mod unknown_type;
pub mod unnecessary_assert;
pub mod unnecessary_negation;
pub mod unreachable_code;
pub mod unused_argument;
pub mod unused_label;
pub mod unused_loop_variable;
pub mod unused_variable;
pub mod value_overwritten_before_read;

use crate::rule::Rule;

/// How a rule participates in the lint pass.
pub enum RuleEntry {
    /// Walks the whole tree itself in `Rule::check`.
    Whole(&'static dyn Rule),
    /// Driven by the shared single-pass bus walk. Carries both vtables
    /// because dyn-upcasting needs Rust 1.86 and MSRV is 1.85.
    Node(&'static dyn Rule, &'static dyn crate::rule::NodeRule),
}

impl RuleEntry {
    pub fn rule(&self) -> &'static dyn Rule {
        match self {
            RuleEntry::Whole(rule) | RuleEntry::Node(rule, _) => *rule,
        }
    }
}

/// Every registered rule, in registration order. Rules are stateless
/// units (configuration reaches them through `LintContext::config`),
/// so the registry is a static: no per-lint boxing or cloning.
pub static RULES: &[RuleEntry] = &[
    RuleEntry::Whole(&undefined_variable::UndefinedVariable),
    RuleEntry::Whole(&unused_variable::UnusedVariable),
    RuleEntry::Whole(&setting_global::SettingGlobal),
    RuleEntry::Node(
        &duplicate_keys::DuplicateKeys,
        &duplicate_keys::DuplicateKeys,
    ),
    RuleEntry::Node(&compare_nan::CompareNan, &compare_nan::CompareNan),
    RuleEntry::Node(
        &constant_table_comparison::ConstantTableComparison,
        &constant_table_comparison::ConstantTableComparison,
    ),
    RuleEntry::Whole(&almost_swapped::AlmostSwapped),
    RuleEntry::Node(
        &type_check_inside_call::TypeCheckInsideCall,
        &type_check_inside_call::TypeCheckInsideCall,
    ),
    RuleEntry::Node(
        &incorrect_stdlib_use::IncorrectStdlibUse,
        &incorrect_stdlib_use::IncorrectStdlibUse,
    ),
    RuleEntry::Node(&deprecated::Deprecated, &deprecated::Deprecated),
    RuleEntry::Node(
        &duplicate_conditions::DuplicateConditions,
        &duplicate_conditions::DuplicateConditions,
    ),
    RuleEntry::Whole(&invalid_lint_filter::InvalidLintFilter),
    RuleEntry::Node(&for_range::ForRange, &for_range::ForRange),
    RuleEntry::Whole(&bad_string_escape::BadStringEscape),
    RuleEntry::Node(
        &comparison_precedence::ComparisonPrecedence,
        &comparison_precedence::ComparisonPrecedence,
    ),
    RuleEntry::Node(
        &integer_parsing::IntegerParsing,
        &integer_parsing::IntegerParsing,
    ),
    RuleEntry::Node(&divide_by_zero::DivideByZero, &divide_by_zero::DivideByZero),
    RuleEntry::Node(
        &misleading_and_or::MisleadingAndOr,
        &misleading_and_or::MisleadingAndOr,
    ),
    RuleEntry::Whole(&implicit_return::ImplicitReturn),
    RuleEntry::Node(&format_string::FormatString, &format_string::FormatString),
    RuleEntry::Whole(&value_overwritten_before_read::ValueOverwrittenBeforeRead),
    RuleEntry::Whole(&accessing_uninitialized::AccessingUninitialized),
    RuleEntry::Whole(&mutating_uninitialized::MutatingUninitialized),
    RuleEntry::Node(
        &restricted_module_paths::RestrictedModulePaths,
        &restricted_module_paths::RestrictedModulePaths,
    ),
    RuleEntry::Node(&empty_block::EmptyBlock, &empty_block::EmptyBlock),
    RuleEntry::Node(
        &reversed_for_loop::ReversedForLoop,
        &reversed_for_loop::ReversedForLoop,
    ),
    RuleEntry::Node(
        &unbalanced_assignment::UnbalancedAssignment,
        &unbalanced_assignment::UnbalancedAssignment,
    ),
    RuleEntry::Node(
        &if_same_then_else::IfSameThenElse,
        &if_same_then_else::IfSameThenElse,
    ),
    RuleEntry::Whole(&unreachable_code::UnreachableCode),
    RuleEntry::Node(&must_use::MustUse, &must_use::MustUse),
    RuleEntry::Whole(&placeholder_read::PlaceholderRead),
    RuleEntry::Node(&mixed_table::MixedTable, &mixed_table::MixedTable),
    RuleEntry::Whole(&loop_executes_once::LoopExecutesOnce),
    RuleEntry::Whole(&shadowing::Shadowing),
    RuleEntry::Node(
        &parenthesized_conditions::ParenthesizedConditions,
        &parenthesized_conditions::ParenthesizedConditions,
    ),
    RuleEntry::Node(
        &redundant_nil_init::RedundantNilInit,
        &redundant_nil_init::RedundantNilInit,
    ),
    RuleEntry::Whole(&string_index_to_field::StringIndexToField),
    RuleEntry::Whole(&merge_adjacent_locals::MergeAdjacentLocals),
    RuleEntry::Whole(&unused_label::UnusedLabel),
    RuleEntry::Whole(&unused_argument::UnusedArgument),
    RuleEntry::Whole(&unused_loop_variable::UnusedLoopVariable),
    RuleEntry::Whole(&set_but_never_read::SetButNeverRead),
    RuleEntry::Whole(&redefining_local::RedefiningLocal),
    RuleEntry::Whole(&multiple_statements_per_line::MultipleStatementsPerLine),
    RuleEntry::Whole(&cyclomatic_complexity::CyclomaticComplexity),
    RuleEntry::Whole(&global_usage::GlobalUsage),
    RuleEntry::Whole(&manual_table_clone::ManualTableClone),
    RuleEntry::Whole(&mismatched_arg_count::MismatchedArgCount),
    RuleEntry::Whole(&global_used_as_local::GlobalUsedAsLocal),
    RuleEntry::Whole(&builtin_global_write::BuiltinGlobalWrite),
    RuleEntry::Whole(&duplicate_function::DuplicateFunction),
    RuleEntry::Whole(&redundant_return::RedundantReturn),
    RuleEntry::Whole(&comment_directive::CommentDirective),
    RuleEntry::Node(
        &redundant_native_attribute::RedundantNativeAttribute,
        &redundant_native_attribute::RedundantNativeAttribute,
    ),
    RuleEntry::Node(
        &unnecessary_negation::UnnecessaryNegation,
        &unnecessary_negation::UnnecessaryNegation,
    ),
    RuleEntry::Node(
        &duplicate_parameter::DuplicateParameter,
        &duplicate_parameter::DuplicateParameter,
    ),
    RuleEntry::Node(
        &table_operations::TableOperations,
        &table_operations::TableOperations,
    ),
    RuleEntry::Node(
        &unnecessary_assert::UnnecessaryAssert,
        &unnecessary_assert::UnnecessaryAssert,
    ),
    RuleEntry::Node(
        &ambiguous_newline_call::AmbiguousNewlineCall,
        &ambiguous_newline_call::AmbiguousNewlineCall,
    ),
    RuleEntry::Node(&unknown_type::UnknownType, &unknown_type::UnknownType),
    RuleEntry::Node(
        &roblox_incorrect_color3_new_bounds::RobloxIncorrectColor3NewBounds,
        &roblox_incorrect_color3_new_bounds::RobloxIncorrectColor3NewBounds,
    ),
    RuleEntry::Node(
        &roblox_suspicious_udim2_new::RobloxSuspiciousUdim2New,
        &roblox_suspicious_udim2_new::RobloxSuspiciousUdim2New,
    ),
    RuleEntry::Node(
        &roblox_manual_fromscale_or_fromoffset::RobloxManualFromScaleOrFromOffset,
        &roblox_manual_fromscale_or_fromoffset::RobloxManualFromScaleOrFromOffset,
    ),
];

pub fn all_rules() -> Vec<&'static dyn Rule> {
    RULES.iter().map(RuleEntry::rule).collect()
}

/// Synthetic pseudo-rule name for parse errors. Not a real `Rule`, but
/// callers (suppression directives, config validation) treat it as a
/// suppressible rule name, so it joins the registered set.
const PARSE_ERROR_PSEUDO_RULE: &str = "parse_error";

/// Names of every rule the linter knows about, in registration order,
/// plus the synthetic `parse_error` pseudo-rule. Derived from
/// `all_rules` so the two lists cannot drift. The full set is enumerated
/// with a default config; `all_rules` only filters on config-driven
/// fields (not presence of a rule), so every rule object is always
/// constructed regardless of config. Used by `invalid_lint_filter` and
/// `unknown_rule_names` to validate suppression directives and overrides.
pub fn registered_rule_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = all_rules().iter().map(|rule| rule.name()).collect();
    names.push(PARSE_ERROR_PSEUDO_RULE);
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_names_match_all_rules() {
        // `registered_rule_names` is derived from `all_rules`, so the two
        // cannot drift; this asserts the relationship explicitly. Every
        // `all_rules` name must appear, and the only extra entry is the
        // synthetic `parse_error` pseudo-rule.
        let _config = crate::LintConfig::default();
        let live: Vec<&str> = all_rules().iter().map(|r| r.name()).collect();
        let listed = registered_rule_names();
        for name in &live {
            assert!(
                listed.contains(name),
                "rule `{name}` is in all_rules() but missing from registered_rule_names()"
            );
        }
        assert_eq!(
            listed.len(),
            live.len() + 1,
            "registered_rule_names() must be exactly all_rules() plus `parse_error`"
        );
        assert!(listed.contains(&PARSE_ERROR_PSEUDO_RULE));
    }

    #[test]
    fn rule_count_locked() {
        // If this changes, update the rule count in README.md,
        // crates/luck_linter/README.md, and CLAUDE.md.
        assert_eq!(all_rules().len(), 63);
    }

    #[test]
    fn registered_names_unique() {
        let names = registered_rule_names();
        let mut seen = std::collections::HashSet::new();
        for name in &names {
            assert!(
                seen.insert(*name),
                "duplicate name `{name}` in registered list"
            );
        }
    }
}
