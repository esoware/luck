use luck_ast::{Expression, Statement};

use crate::cfg::decision_points;
use crate::diagnostic::Finding;
use crate::function_node::FunctionNode;
use crate::node_type::NodeTypeSet;
use crate::rule::{LintContext, NodeRule, impl_rule};

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

impl NodeRule for CyclomaticComplexity {
    fn node_types(&self) -> NodeTypeSet {
        FunctionNode::NODE_TYPES
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        findings.extend(
            FunctionNode::of_stmt(stmt).and_then(|function| check_function(function, context)),
        );
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        findings.extend(
            FunctionNode::of_expr(expr).and_then(|function| check_function(function, context)),
        );
    }
}

/// Flags `function` when its complexity is over the configured threshold.
fn check_function(function: FunctionNode<'_>, context: &LintContext<'_>) -> Option<Finding> {
    let threshold = context.config.max_cyclomatic_complexity?;
    // McCabe: one straight-line path plus one per decision point.
    let complexity = decision_points(&function.body.block).saturating_add(1);
    (complexity > threshold).then(|| {
        // Slice the name from source so `mod.f` and `a.b:c` appear whole.
        let label = function.name_span.map_or_else(
            || "(anonymous)".to_owned(),
            |name_span| format!("`{}`", name_span.text(context.source)),
        );
        Finding::new(
            format!(
                "function {label} has cyclomatic complexity {complexity} (threshold: {threshold})"
            ),
            function.span,
        )
        .with_help("split this function into smaller helpers")
    })
}

#[cfg(test)]
mod tests {
    use luck_core::LintConfig;
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule_with_config;

    fn run(source: &str, threshold: Option<u32>) -> Vec<LintDiagnostic> {
        let config = LintConfig { max_cyclomatic_complexity: threshold, ..LintConfig::default() };
        run_rule_with_config(&CyclomaticComplexity, source, LuaVersion::Lua54, &config)
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
        let source = "local m = {}\nfunction m.sub:run() if a then end end";
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
        let config = LintConfig { max_cyclomatic_complexity: Some(0), ..LintConfig::default() };
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
        let config = LintConfig { max_cyclomatic_complexity: Some(1), ..LintConfig::default() };
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
