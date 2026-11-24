use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UndefinedVariable;

impl_rule!(
    UndefinedVariable,
    "undefined_variable",
    Correctness,
    Error,
    "use of undefined variable"
);

impl RuleRunner for UndefinedVariable {
    fn run_once(&self, context: &LintContext<'_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();
        for reference in tree.global_references() {
            // `setting_global` owns writes, and a global this file writes
            // (`function f() end`, `counter = 0`) counts as defined
            // everywhere in it, or script-style Lua would error on every use.
            if reference.kind.is_write()
                || semantic.is_defined_global(reference)
                || tree.name(reference.name) == "_"
                || tree.is_assigned_global(reference.name)
            {
                continue;
            }
            let name = tree.name(reference.name);
            context.diagnostic(
                Finding::new(format!("undefined variable `{name}`"), reference.span).with_help(
                    "declare it with `local`, or add it to `extra_globals` or a `std` definition",
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
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
            "\
global x = 1
print(x)",
            "\
global function f() end
f()",
            "\
global y
print(y)",
            "\
global z <const> = 2
print(z)",
        ] {
            let diags = run_rule(&UndefinedVariable, source, LuaVersion::Lua55);
            assert!(diags.is_empty(), "{source}: {diags:?}");
        }
    }

    #[test]
    fn ignores_global_the_file_assigns() {
        let diags = run("\
print(counter)
counter = 0
function helper() end
helper()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_plain_write_and_discard() {
        let diags = run("\
fresh = 1
print(_)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_compound_assignment_to_undeclared_name() {
        let diags = run_rule(&UndefinedVariable, "counter += 1", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_read_through_local_env() {
        let diags = run("\
local _ENV = { y = 1 }
return y");
        assert!(diags.is_empty(), "`y` is a field of the local `_ENV`: {diags:?}");
    }

    #[test]
    fn ignores_read_after_global_star() {
        let diags = run_rule(
            &UndefinedVariable,
            "\
global *
return y",
            LuaVersion::Lua55,
        );
        assert!(diags.is_empty(), "`global *` declares every free name: {diags:?}");
    }

    #[test]
    fn ignores_types_library_in_type_function() {
        let source = "\
type function t1(a)
 return types.string
end";
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
