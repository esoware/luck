use luck_semantic::scope::ReferenceKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct SettingGlobal;

impl Rule for SettingGlobal {
    fn name(&self) -> &'static str {
        "setting_global"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "implicit global variable assignment (missing `local`?)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let semantic = ctx.semantic;
        let mut diagnostics = Vec::new();

        for reference in semantic.scope_tree.unresolved_references() {
            if !matches!(
                reference.kind,
                ReferenceKind::Write | ReferenceKind::ReadWrite
            ) {
                continue;
            }
            // Reassigning a known global (e.g. `print = ...`) is weird but valid Lua.
            if semantic.is_known_global(&reference.name) {
                continue;
            }
            // `_` is a conventional discard name, not an accidental global.
            if reference.name == "_" {
                continue;
            }

            diagnostics.push(
                LintDiagnostic::new(
                    "setting_global",
                    format!(
                        "setting global variable `{}` (missing `local`?)",
                        reference.name
                    ),
                    reference.span,
                )
                .with_help("add `local` before the first assignment".to_string()),
            );
        }

        diagnostics
    }
}
