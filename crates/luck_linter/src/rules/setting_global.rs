use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingGlobal;

impl_rule!(
    SettingGlobal,
    "setting_global",
    Correctness,
    Warning,
    "implicit global variable assignment (missing `local`?)"
);

impl WholeRule for SettingGlobal {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let mut findings = Vec::new();

        for reference in semantic.scope_tree.unresolved_references() {
            if !reference.kind.is_write() {
                continue;
            }
            // Writing a known global like `print` isn't a forgotten `local`.
            if semantic.is_defined_global(reference) {
                continue;
            }
            // `_` is a conventional discard name, not an accidental global.
            if reference.name == "_" {
                continue;
            }

            findings.push(
                Finding::new(
                    format!("setting global variable `{}` (missing `local`?)", reference.name),
                    reference.span,
                )
                .with_help("add `local` before the first assignment"),
            );
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
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
    fn ignores_declared_global_and_its_writes() {
        for source in ["global x = 1", "global x\nx = 1", "global function f() end\nf = nil"] {
            let diags = run_rule(&SettingGlobal, source, LuaVersion::Lua55);
            assert!(diags.is_empty(), "{source}: {diags:?}");
        }
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

    #[test]
    fn ignores_write_through_local_env() {
        let diags = run("local _ENV = {}\nx = 1");
        assert!(diags.is_empty(), "`x` is a field of the local `_ENV`: {diags:?}");
    }

    #[test]
    fn ignores_write_after_global_star() {
        let diags = run_rule(&SettingGlobal, "global *\nx = 1", LuaVersion::Lua55);
        assert!(diags.is_empty(), "`global *` declares every free name: {diags:?}");
    }
}
