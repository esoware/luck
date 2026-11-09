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
        semantic
            .scope_tree
            .unresolved_references()
            .filter(|reference| {
                // `setting_global` owns writes, and a global this file writes
                // (`function f() end`, `counter = 0`) counts as defined
                // everywhere in it, or script-style Lua would error on every
                // use.
                !reference.kind.is_write()
                    && !semantic.is_defined_global(reference)
                    && reference.name != "_"
                    && !semantic.scope_tree.assigned_globals.contains(&reference.name)
            })
            .map(|reference| {
                Finding::new(format!("undefined variable `{}`", reference.name), reference.span)
                    .with_help("declare it with `local`, or add it to `extra_globals`")
            })
            .collect()
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
    fn ignores_read_through_local_env() {
        let diags = run("local _ENV = { y = 1 }\nreturn y");
        assert!(diags.is_empty(), "`y` is a field of the local `_ENV`: {diags:?}");
    }

    #[test]
    fn ignores_read_after_global_star() {
        let diags = run_rule(&UndefinedVariable, "global *\nreturn y", LuaVersion::Lua55);
        assert!(diags.is_empty(), "`global *` declares every free name: {diags:?}");
    }

    #[test]
    fn ignores_types_library_in_type_function() {
        let source = "type function t1(a)\n return types.string\nend";
        let diags = run_rule(&UndefinedVariable, source, LuaVersion::Luau);
        assert!(diags.is_empty(), "type functions have the `types` library: {diags:?}");
    }

    #[test]
    fn ignores_implicit_arg_table_of_a_lua51_vararg_function() {
        let diags =
            run_rule(&UndefinedVariable, "local function f(...) return arg end", LuaVersion::Lua51);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
