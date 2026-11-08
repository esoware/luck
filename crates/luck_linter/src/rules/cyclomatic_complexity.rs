use luck_ast::{Expression, Statement};

use crate::cfg::analyze_full_block;
use crate::diagnostic::{Category, Finding, Severity};
use crate::function_node::FunctionNode;
use crate::node_type::NodeTypeSet;
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CyclomaticComplexity;

impl Rule for CyclomaticComplexity {
    fn name(&self) -> &'static str {
        "cyclomatic_complexity"
    }
    fn category(&self) -> Category {
        // `Category` has no complexity variant, and `Style` fits pressure
        // to refactor best.
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "function exceeds the configured cyclomatic complexity threshold"
    }
}

impl NodeRule for CyclomaticComplexity {
    fn node_types(&self) -> NodeTypeSet {
        FunctionNode::NODE_TYPES
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        findings.extend(
            FunctionNode::of_statement(stmt).and_then(|function| check_function(function, context)),
        );
    }
    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        findings.extend(
            FunctionNode::of_expression(expr)
                .and_then(|function| check_function(function, context)),
        );
    }
}

/// Flags `function` when its complexity is over the configured threshold.
fn check_function(function: FunctionNode<'_>, context: &LintContext<'_>) -> Option<Finding> {
    let threshold = context.config.max_cyclomatic_complexity?;
    // McCabe: one straight-line path plus one per decision point.
    let complexity = analyze_full_block(&function.body.block).decision_points.saturating_add(1);
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
