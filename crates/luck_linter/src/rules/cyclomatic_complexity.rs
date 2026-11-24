use luck_semantic::{AstNode, AstTypesBitset};

use crate::cfg::decision_points;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::function_node::FunctionNode;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CyclomaticComplexity;

// `Category` has no complexity variant, and `Style` fits pressure to refactor best.
impl_rule!(
    CyclomaticComplexity,
    "cyclomatic_complexity",
    Style,
    Warning,
    "function exceeds the configured cyclomatic complexity threshold"
);

impl RuleRunner for CyclomaticComplexity {
    fn node_types(&self) -> AstTypesBitset {
        FunctionNode::NODE_TYPES
    }

    fn should_run(&self, host: &ContextHost<'_>) -> bool {
        threshold(host).is_some()
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let (Some(threshold), Some(function)) =
            (threshold(context), FunctionNode::of(node, context.nodes))
        else {
            return;
        };
        // McCabe: one straight-line path plus one per decision point.
        let complexity = decision_points(&function.body.block).saturating_add(1);
        if complexity <= threshold {
            return;
        }
        // Slice the name from source so `mod.f` and `a.b:c` appear whole.
        let label = function.name_span.map_or_else(
            || "(anonymous)".to_owned(),
            |name_span| format!("`{}`", String::from_utf8_lossy(name_span.text(context.source))),
        );
        context.diagnostic(
            Finding::new(
                format!(
                    "function {label} has cyclomatic complexity {complexity} (threshold: {threshold})"
                ),
                function.span,
            )
            .with_help("split this function into smaller helpers"),
        );
    }
}

/// The configured threshold. With none, the rule checks nothing.
fn threshold(host: &ContextHost<'_>) -> Option<u32> {
    Some(host.config.rules.cyclomatic_complexity.as_ref()?.options()?.max)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::config::{CyclomaticComplexityOptions, LintConfig, RuleLevel, RuleSetting};
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule_with_config;

    fn config(threshold: Option<u32>) -> LintConfig {
        let mut config = LintConfig::default();
        config.rules.cyclomatic_complexity =
            Some(threshold.map_or(RuleSetting::Level(RuleLevel::Warn), |max| {
                RuleSetting::WithOptions(RuleLevel::Warn, CyclomaticComplexityOptions { max })
            }));
        config
    }

    fn run(source: &str, threshold: Option<u32>) -> Vec<Message> {
        run_rule_with_config(&CyclomaticComplexity, source, LuaVersion::Lua54, &config(threshold))
    }

    #[test]
    fn flags_when_complexity_exceeds_threshold() {
        // Four `if` statements = 4 decision points + 1 baseline = 5.
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = run(source, Some(3));
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("complexity 5"), "msg: {}", diags[0].message);
    }

    #[test]
    fn ignores_when_threshold_is_none() {
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = run(source, None);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_when_threshold_is_high() {
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = run(source, Some(100));
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_named_function_by_name() {
        let source = "function my_func() if a then end if b then end end";
        let diags = run(source, Some(1));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags[0].message,
            "function `my_func` has cyclomatic complexity 3 (threshold: 1)"
        );
    }

    #[test]
    fn flags_method_by_its_full_name() {
        let source = "\
local m = {}
function m.sub:run() if a then end end";
        let diags = run(source, Some(1));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`m.sub:run`"), "{diags:?}");
    }

    #[test]
    fn flags_anonymous_function_as_anonymous() {
        let source = "local f = function() if a then end if b then end end";
        let diags = run(source, Some(1));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("(anonymous)"), "msg: {}", diags[0].message);
    }

    #[test]
    fn flags_decision_points_in_table_call_arguments() {
        for source in [
            "function paren_table_arg() f({a and b}) end",
            "function bare_table_arg() f{a and b} end",
        ] {
            let diags = run(source, Some(0));
            assert!(diags[0].message.contains("complexity 2"), "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_decision_points_in_assignment_targets() {
        for source in
            ["function value_and() x = a and b end", "function target_or() t[a or b] = 1 end"]
        {
            let diags = run(source, Some(0));
            assert!(diags[0].message.contains("complexity 2"), "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_decision_points_in_return_exprs() {
        let diags = run("function return_and() return a and b end", Some(0));
        assert!(diags[0].message.contains("complexity 2"), "{diags:?}");
    }

    #[test]
    fn flags_decision_points_in_compound_assignment_targets() {
        let config = config(Some(0));
        for source in [
            "function comp_value() t[1] += a or b end",
            "function comp_target() t[a or b] += 1 end",
        ] {
            let diags =
                run_rule_with_config(&CyclomaticComplexity, source, LuaVersion::Luau, &config);
            assert!(diags[0].message.contains("complexity 2"), "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_type_function_bodies() {
        let config = config(Some(1));
        let source = "type function t1(a) if a then return a end end";
        let diags = run_rule_with_config(&CyclomaticComplexity, source, LuaVersion::Luau, &config);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "function `t1` has cyclomatic complexity 2 (threshold: 1)");
    }

    #[test]
    fn flags_empty_function_over_zero_threshold() {
        let source = "function f() end";
        let diags = run(source, Some(0));
        // Baseline 1 > 0, so the threshold trips.
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_each_function_independently() {
        let source = "function f() if a then end if b then end end function g() if c then end end";
        let diags = run(source, Some(1));
        // `f` has complexity 3 (>1); `g` has complexity 2 (>1). Both fire.
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }
}
