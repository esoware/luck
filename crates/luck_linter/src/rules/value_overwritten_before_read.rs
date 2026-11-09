use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags a value written to a local that some path overwrites and no path
/// reads first (Luacheck 311).
///
/// A captured local is skipped, since a closure can read it at any time.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ValueOverwrittenBeforeRead;

impl_rule!(
    ValueOverwrittenBeforeRead,
    "value_overwritten_before_read",
    Correctness,
    Warning,
    "value assigned to a local is overwritten before it is read"
);

impl WholeRule for ValueOverwrittenBeforeRead {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let tree = &context.semantic.scope_tree;
        context
            .local_flow()
            .overwritten_values
            .iter()
            .map(|&(symbol_id, span)| {
                Finding::new(
                    format!(
                        "value assigned to `{}` is overwritten before it is read",
                        tree.symbol(symbol_id).name
                    ),
                    span,
                )
                .with_help("remove the earlier assignment or read it first")
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
        run_rule(&ValueOverwrittenBeforeRead, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_initializer_then_overwrite() {
        let diags = run("local x = 1\nx = 2\nreturn x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_branched_overwrite() {
        let diags = run("local x = 1\nif c then x = 2 end\nreturn x");
        assert!(diags.is_empty(), "branched writes must not fire (conservative): {diags:?}");
    }

    #[test]
    fn ignores_name_the_initializer_list_leaves_without_a_value() {
        let diags = run("local a, b = 1
b = 2
return a, b");
        assert!(diags.is_empty(), "`local a, b = 1` assigns `b` no value: {diags:?}");
    }

    #[test]
    fn ignores_read_between_writes() {
        let diags = run("local x = 1\nprint(x)\nx = 2\nreturn x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_single_initializer() {
        let diags = run("local x = 1\nreturn x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_two_writes_after_declaration() {
        // Write then write with no read between, so the first is dead.
        let diags = run("local x\nx = 1\nx = 2\nreturn x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x = 1\n_x = 2\nreturn _x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_assignment_reading_its_own_target() {
        let diags = run("local x = 1\nx = x + 1\nreturn x");
        assert!(diags.is_empty(), "the right-hand side reads the old value: {diags:?}");
    }

    #[test]
    fn ignores_write_skipped_by_goto() {
        let diags = run("local x = 1\nif c then goto skip end\nx = 2\n::skip::\nprint(x)");
        assert!(diags.is_empty(), "the `goto` path reads the initializer: {diags:?}");
    }

    #[test]
    fn flags_the_overwritten_assignment_at_its_name() {
        let source = "local x\nx = 1\nx = 2\nreturn x";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].span.text(source), "x", "the span should cover `x`: {diags:?}");
    }

    #[test]
    fn flags_initializer_overwritten_on_every_branch() {
        let diags = run("local x = 1\nif c then x = 2 else x = 3 end\nreturn x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_value_read_in_the_next_iteration() {
        let diags = run("local x = 1\nfor _ = 1, 3 do print(x)\nx = 2 end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
