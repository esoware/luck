use rustc_hash::FxHashSet;

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UndefinedVariable;

impl_rule!(
    UndefinedVariable,
    "undefined_variable",
    Correctness,
    Error,
    "use of undefined variable"
);

impl WholeRule for UndefinedVariable {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let mut findings = Vec::new();

        // Globals this file assigns (`function f() end`, `counter = 0`)
        // count as defined everywhere in it, or script-style Lua would
        // error on every use.
        let assigned_globals: FxHashSet<&str> = semantic
            .scope_tree
            .unresolved_references()
            .filter(|reference| reference.kind.is_write())
            .map(|reference| reference.name.as_str())
            .collect();

        for reference in semantic.scope_tree.unresolved_references() {
            // `setting_global` owns writes to undeclared names.
            if reference.kind.is_write() {
                continue;
            }
            if semantic.is_known_global(&reference.name) {
                continue;
            }
            if reference.name == "_" {
                continue;
            }
            if assigned_globals.contains(reference.name.as_str()) {
                continue;
            }

            findings.push(
                Finding::new(format!("undefined variable `{}`", reference.name), reference.span)
                    .with_help("declare it with `local`, or add it to `extra_globals`"),
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
        run_rule(&UndefinedVariable, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_read_of_undeclared_name() {
        let diags = run("print(missing)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`missing`"), "{diags:?}");
    }

    #[test]
    fn ignores_names_declared_with_global() {
        for source in [
            "global x = 1\nprint(x)",
            "global function f() end\nf()",
            "global y\nprint(y)",
            "global z <const> = 2\nprint(z)",
        ] {
            let diags = run_rule(&UndefinedVariable, source, LuaVersion::Lua55);
            assert!(diags.is_empty(), "{source}: {diags:?}");
        }
    }

    #[test]
    fn ignores_global_the_file_assigns() {
        let diags = run("print(counter)\ncounter = 0\nfunction helper() end\nhelper()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_plain_write_and_discard() {
        let diags = run("fresh = 1\nprint(_)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_compound_assignment_to_undeclared_name() {
        let diags = run_rule(&UndefinedVariable, "counter += 1", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn bug_ignores_read_through_local_env() {
        let diags = run("local _ENV = { y = 1 }\nreturn y");
        assert!(diags.is_empty(), "`y` is a field of the local `_ENV`: {diags:?}");
    }

    #[test]
    fn bug_ignores_read_after_global_star() {
        let diags = run_rule(&UndefinedVariable, "global *\nreturn y", LuaVersion::Lua55);
        assert!(diags.is_empty(), "`global *` declares every free name: {diags:?}");
    }

    #[test]
    fn bug_ignores_types_library_in_type_function() {
        let source = "type function t1(a)\n return types.string\nend";
        let diags = run_rule(&UndefinedVariable, source, LuaVersion::Luau);
        assert!(diags.is_empty(), "type functions have the `types` library: {diags:?}");
    }
}
