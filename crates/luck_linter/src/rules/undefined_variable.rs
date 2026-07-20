use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct UndefinedVariable;

impl Rule for UndefinedVariable {
    fn name(&self) -> &'static str {
        "undefined_variable"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "use of undefined variable"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let semantic = ctx.semantic;
        let mut diagnostics = Vec::new();

        // Globals defined in this file (`function f() end`, `counter = 0`)
        // are known names for the rest of the file - without this, plain
        // Lua script style errors on both the definition and every use.
        let defined_in_file: std::collections::HashSet<&str> = semantic
            .scope_tree
            .unresolved_references()
            .filter(|reference| {
                matches!(
                    reference.kind,
                    luck_semantic::scope::ReferenceKind::Write
                        | luck_semantic::scope::ReferenceKind::ReadWrite
                )
            })
            .map(|reference| reference.name.as_str())
            .collect();

        for reference in semantic.scope_tree.unresolved_references() {
            if semantic.is_known_global(&reference.name) {
                continue;
            }
            if reference.name == "_" {
                continue;
            }
            // Writes are skipped because setting globals is a different rule.
            if matches!(reference.kind, luck_semantic::scope::ReferenceKind::Write) {
                continue;
            }
            if defined_in_file.contains(reference.name.as_str()) {
                continue;
            }

            diagnostics.push(
                LintDiagnostic::new(
                    "undefined_variable",
                    format!("undefined variable `{}`", reference.name),
                    reference.span,
                )
                .with_help("declare it with `local` or add to globals list".to_string()),
            );
        }

        diagnostics
    }
}
