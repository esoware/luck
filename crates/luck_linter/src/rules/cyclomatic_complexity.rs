use luck_ast::{Expression, FunctionBody, Statement};
use luck_token::Span;

use crate::cfg::analyze_full_block;
use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
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
        NodeTypeSet::from_types(&[
            NodeType::FunctionDecl,
            NodeType::LocalFunction,
            NodeType::GlobalFunction,
            NodeType::FunctionDef,
        ])
    }

    fn on_statement(
        &self,
        stmt: &Statement,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let Some(threshold) = context.config.max_cyclomatic_complexity else {
            return;
        };
        // Slice the name from source so `mod.f` and `a.b:c` appear whole.
        let (body, span, name_span) = match stmt {
            Statement::FunctionDecl(decl) => (&decl.body, decl.span, decl.name.span),
            Statement::LocalFunction(local_function) => {
                (&local_function.body, local_function.span, local_function.name.span)
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                (&global_function.body, global_function.span, global_function.name.span)
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => return,
        };
        findings.extend(check_function(
            body,
            span,
            Some(name_span.text(context.source)),
            threshold,
        ));
    }

    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let Some(threshold) = context.config.max_cyclomatic_complexity else {
            return;
        };
        if let Expression::FunctionDef(function) = expr {
            findings.extend(check_function(&function.body, function.span, None, threshold));
        }
    }
}

/// Flags the function at `span`, named `name` unless it's anonymous, when
/// its complexity is over `threshold`.
fn check_function(
    body: &FunctionBody,
    span: Span,
    name: Option<&str>,
    threshold: u32,
) -> Option<Finding> {
    // McCabe: one straight-line path plus one per decision point.
    let complexity = analyze_full_block(&body.block).decision_points.saturating_add(1);
    (complexity > threshold).then(|| {
        let label = name
            .map_or_else(|| "(anonymous)".to_owned(), |function_name| format!("`{function_name}`"));
        Finding::new(
            format!(
                "function {label} has cyclomatic complexity {complexity} (threshold: {threshold})"
            ),
            span,
        )
        .with_help("split this function into smaller helpers")
    })
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule_with_config;

    fn run(source: &str, threshold: Option<u32>) -> Vec<LintDiagnostic> {
        let config = crate::LintConfig {
            max_cyclomatic_complexity: threshold,
            ..crate::LintConfig::default()
        };
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
