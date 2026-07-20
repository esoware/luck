use luck_ast::Expression;
use luck_ast::shared::FunctionBody;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;
use luck_token::Span;

use crate::cfg::analyze_full_block;
use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct CyclomaticComplexity;

impl Rule for CyclomaticComplexity {
    fn name(&self) -> &'static str {
        "cyclomatic_complexity"
    }
    fn category(&self) -> Category {
        // The diagnostic crate has no `Complexity` variant. `Style` is
        // the closest fit since complexity warnings are stylistic
        // pressure to refactor, not correctness or performance bugs.
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "function exceeds configured cyclomatic complexity threshold"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let source = ctx.source;
        let Some(threshold) = ctx.config.max_cyclomatic_complexity else {
            return Vec::new();
        };
        let mut checker = ComplexityChecker {
            source,
            threshold,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct ComplexityChecker<'src> {
    source: &'src str,
    threshold: u32,
    diagnostics: Vec<LintDiagnostic>,
}

impl<'src> ComplexityChecker<'src> {
    fn check_function(&mut self, body: &FunctionBody, span: Span, name: Option<&str>) {
        let summary = analyze_full_block(&body.block);
        // McCabe baseline of 1 is the single straight-line path; each
        // decision point adds an independent path.
        let complexity = summary.decision_points + 1;
        if complexity <= self.threshold {
            return;
        }
        let label = name
            .map(|n| format!("`{n}`"))
            .unwrap_or_else(|| "anonymous function".to_string());
        self.diagnostics.push(
            LintDiagnostic::new(
                "cyclomatic_complexity",
                format!(
                    "function {label} has cyclomatic complexity {complexity} (threshold: {})",
                    self.threshold
                ),
                span,
            )
            .with_help("split this function into smaller helpers".to_string()),
        );
    }

    fn slice(&self, span: Span) -> &'src str {
        &self.source[span.start as usize..span.end as usize]
    }
}

impl<'ast> Visitor<'ast> for ComplexityChecker<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::FunctionDecl(decl) => {
                // Re-slice the full dotted name from source so methods
                // like `mod.f` or `a.b:c` appear intact in diagnostics.
                let name = self.slice(decl.name.span).to_string();
                self.check_function(&decl.body, decl.span, Some(&name));
            }
            Statement::LocalFunction(local) => {
                let name = self.slice(local.name.span).to_string();
                self.check_function(&local.body, local.span, Some(&name));
            }
            Statement::GlobalFunction(global) => {
                let name = self.slice(global.name.span).to_string();
                self.check_function(&global.body, global.span, Some(&name));
            }
            _ => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionDef(func_def) = expr {
            self.check_function(&func_def.body, func_def.span, None);
        }
        self.walk_expression(expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str, threshold: Option<u32>) -> Vec<LintDiagnostic> {
        let config = crate::LintConfig {
            max_cyclomatic_complexity: threshold,
            ..crate::LintConfig::default()
        };
        crate::test_support::run_rule_with_config(
            &CyclomaticComplexity,
            source,
            LuaVersion::Lua54,
            &config,
        )
    }

    #[test]
    fn flags_when_complexity_exceeds_threshold() {
        // Four `if` statements = 4 decision points + 1 baseline = 5.
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = run(source, Some(3));
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0].message.contains("complexity 5"),
            "msg: {}",
            diags[0].message
        );
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
    fn includes_function_name_in_message() {
        let source = "function my_func() if a then end if b then end end";
        let diags = run(source, Some(1));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].message.contains("`my_func`"),
            "msg: {}",
            diags[0].message
        );
    }

    #[test]
    fn anonymous_function_labeled_as_such() {
        let source = "local f = function() if a then end if b then end end";
        let diags = run(source, Some(1));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].message.contains("anonymous"),
            "msg: {}",
            diags[0].message
        );
    }

    #[test]
    fn empty_function_has_complexity_one() {
        let source = "function f() end";
        let diags = run(source, Some(0));
        // Baseline 1 > 0, so the threshold trips.
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn each_function_scanned_independently() {
        let source = "function f() if a then end if b then end end function g() if c then end end";
        let diags = run(source, Some(1));
        // `f` has complexity 3 (>1); `g` has complexity 2 (>1). Both fire.
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }
}
