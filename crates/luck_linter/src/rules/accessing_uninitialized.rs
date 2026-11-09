use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};
use crate::uninitialized::first_reference;

/// Luacheck 321: a local declared without a value is read before anything
/// is assigned to it.
///
/// Only a bare `local x` counts. `local x = nil` is initialized, and
/// `redundant_nil_init` covers it instead. A field or index write through the
/// local is `mutating_uninitialized`'s (Luacheck 341).
#[derive(Debug, Clone, Copy)]
pub(crate) struct AccessingUninitialized;

impl_rule!(
    AccessingUninitialized,
    "accessing_uninitialized",
    Correctness,
    Warning,
    "local is read before any value is assigned to it"
);

impl WholeRule for AccessingUninitialized {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let tree = &context.semantic.scope_tree;
        let uninitialized_locals = context.uninitialized_locals;
        tree.symbols
            .iter()
            .filter(|symbol| uninitialized_locals.contains(symbol) && !symbol.name.starts_with('_'))
            .filter_map(|symbol| {
                let first = first_reference(symbol, tree)?;
                let is_read =
                    first.kind.is_read() && !uninitialized_locals.is_field_write_prefix(first);
                is_read.then(|| {
                    Finding::new(
                        format!(
                            "`{}` is read before any value is assigned, so it is `nil`",
                            symbol.name
                        ),
                        first.span,
                    )
                    .with_help("initialize at declaration or assign before reading")
                })
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
        run_rule(&AccessingUninitialized, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_read_before_assign() {
        let diags = run("local x\nprint(x)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_explicit_nil_init() {
        // `redundant_nil_init` may flag this, but it counts as initialized.
        let diags = run("local x = nil\nprint(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_write_then_read() {
        let diags = run("local x\nx = 1\nprint(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_write_in_branch_before_read() {
        let diags = run("local x\nif c then x = 1 end\nprint(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_field_write_as_the_first_reference() {
        let diags = run("local x\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_index_write_as_the_first_reference() {
        let diags = run("local y\ny[1] = 2");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_field_read_through_a_nested_write_target() {
        // `x.y.z = 1` reads `x.y` first, which isn't a write through `x`.
        let diags = run("local x\nx.y.z = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x\nprint(_x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn bug_ignores_forward_declaration_read_in_closure() {
        let diags = run(
            "local helper\nlocal function main() return helper() end\nhelper = function() return 1 end\nreturn main()",
        );
        assert!(diags.is_empty(), "the closure runs after the assignment: {diags:?}");
    }

    #[test]
    fn bug_ignores_read_of_value_written_in_previous_iteration() {
        let diags =
            run("local last\nfor _, v in ipairs({1}) do if last then print(last) end last = v end");
        assert!(diags.is_empty(), "later iterations read the written value: {diags:?}");
    }

    #[test]
    fn bug_ignores_read_after_backward_goto() {
        let diags =
            run("local x\ngoto set\n::use::\nprint(x)\ndo return end\n::set::\nx = 1\ngoto use");
        assert!(diags.is_empty(), "the assignment runs before the read: {diags:?}");
    }

    #[test]
    fn bug_flags_read_in_own_first_assignment() {
        let diags = run("local x\nx = x + 1\nreturn x");
        assert_eq!(diags.len(), 1, "the right-hand side reads `nil`: {diags:?}");
    }
}
