use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule, WholeRule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingGlobal;

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
}

impl WholeRule for SettingGlobal {
    fn check(&self, context: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let semantic = context.semantic;
        let mut diags = Vec::new();

        for reference in semantic.scope_tree.unresolved_references() {
            if !reference.kind.is_write() {
                continue;
            }
            // Writing a known global like `print` isn't a forgotten `local`.
            if semantic.is_known_global(&reference.name) {
                continue;
            }
            // `_` is a conventional discard name, not an accidental global.
            if reference.name == "_" {
                continue;
            }

            diags.push(
                LintDiagnostic::new(
                    "setting_global",
                    format!("setting global variable `{}` (missing `local`?)", reference.name),
                    reference.span,
                )
                .with_help("add `local` before the first assignment"),
            );
        }

        diags
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&SettingGlobal, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_assignment_to_undeclared_name() {
        let diags = run("counter = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`counter`"), "{diags:?}");
    }

    #[test]
    fn ignores_known_global_and_discard() {
        let diags = run("print = print\n_ = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_local_assignment() {
        let diags = run("local counter\ncounter = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
