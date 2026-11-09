use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};
use crate::uninitialized::first_reference;

/// Flags a field or index write on a local declared without a value, before
/// anything assigns the local itself (Luacheck 341). `local x; x.foo = 1`
/// indexes `nil`.
///
/// The scope tree records the `x` in `x.foo = ...` as a plain read, so
/// `UninitializedLocals` records the write prefixes to tell them apart from
/// `print(x)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MutatingUninitialized;

impl_rule!(
    MutatingUninitialized,
    "mutating_uninitialized",
    Correctness,
    Warning,
    "field or index assignment on an uninitialized local"
);

impl WholeRule for MutatingUninitialized {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let tree = &context.semantic.scope_tree;
        let uninitialized_locals = context.uninitialized_locals;
        tree.symbols
            .iter()
            .filter(|symbol| uninitialized_locals.contains(symbol) && !symbol.name.starts_with('_'))
            .filter_map(|symbol| {
                let first = first_reference(symbol, tree)?;
                uninitialized_locals.is_field_write_prefix(first).then(|| {
                    Finding::new(
                        format!(
                            "field/index assignment on uninitialized `{}` (value is `nil`)",
                            symbol.name
                        ),
                        first.span,
                    )
                    .with_help("initialize the local before mutating its fields")
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
        run_rule(&MutatingUninitialized, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_field_write_on_uninitialized() {
        let diags = run("local x\nx.foo = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn flags_index_write_on_uninitialized() {
        let diags = run("local x\nx[1] = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_when_initialized() {
        let diags = run("local x = {}\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_when_assigned_first() {
        let diags = run("local x\nx = {}\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_read_before_field_write() {
        let diags = run("local x\nprint(x)\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x\n_x.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn bug_ignores_forward_declaration_field_write_in_closure() {
        let diags = run("local M\nlocal function init() M.x = 1 end\nM = {}\ninit()");
        assert!(diags.is_empty(), "the closure runs after the assignment: {diags:?}");
    }
}
