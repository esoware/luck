use luck_ast::{Block, FunctionBody, Statement, Visitor};
use luck_token::Span;

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// A `goto` can't leave its function, so each function body, and the chunk
/// itself, is checked on its own.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnusedLabel;

impl_rule!(
    UnusedLabel,
    "unused_label",
    Style,
    Warning,
    "label declared but never targeted by a `goto` in the same function"
);

impl WholeRule for UnusedLabel {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();
        check_scope(context.block, &mut findings);
        findings
    }
}

fn check_scope(block: &Block, findings: &mut Vec<Finding>) {
    let mut scope = LabelScope::default();
    scope.visit_block(block);

    for (name, span) in &scope.labels {
        // Any `goto` with this name in the same function counts. The parser
        // already enforces label visibility, so matching by name alone
        // avoids false positives on jumps between blocks.
        if !scope.goto_names.iter().any(|goto_name| goto_name == name) {
            findings.push(
                Finding::new(format!("label `{name}` is never targeted by a `goto`"), *span)
                    .with_help("remove the label or add a `goto` that targets it"),
            );
        }
    }

    for body in scope.nested_bodies {
        check_scope(&body.block, findings);
    }
}

#[derive(Default)]
struct LabelScope<'ast> {
    labels: Vec<(&'ast str, Span)>,
    goto_names: Vec<&'ast str>,
    nested_bodies: Vec<&'ast FunctionBody>,
}

impl<'ast> Visitor<'ast> for LabelScope<'ast> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            // Lua 5.2+
            Statement::Label(label) => {
                if let Some(name) = label.name.identifier() {
                    self.labels.push((name, label.span));
                }
            }
            // Lua 5.2+
            Statement::Goto(goto) => {
                if let Some(name) = goto.name.identifier() {
                    self.goto_names.push(name);
                }
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    // A nested function gets its own `check_scope` pass.
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.nested_bodies.push(body);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&UnusedLabel, source, LuaVersion::Lua54)
    }

    #[test]
    fn ignores_label_with_matching_goto() {
        let diags = run("do ::a:: print(1) goto a end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_label_with_no_goto() {
        let diags = run("do ::a:: print(1) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`a`"), "{diags:?}");
    }

    #[test]
    fn flags_label_in_different_function() {
        // g's goto targets g's own label (a cross-function goto would
        // not parse); f's label has no goto anywhere.
        let diags = run("function f() ::a:: print() end function g() ::a:: goto a end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_goto_in_inner_block() {
        let diags = run("do ::a:: do print() goto a end end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_unused_top_level_label() {
        let diags = run("::start:: print(1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_top_level_label_used() {
        let diags = run("::start:: print(1) goto start");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_label_unreachable_from_inner_function() {
        // The inner goto binds to the inner function's own label; the
        // outer label stays unused (labels are not visible across
        // function boundaries).
        let diags = run("::a:: local f = function() ::a:: goto a end");
        assert_eq!(diags.len(), 1, "outer label is unreachable from inner function: {diags:?}");
    }
}
