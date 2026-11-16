use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags a field or index write on a local declared without a value, before
/// any assignment can have reached it (Luacheck 341). `local x; x.foo = 1`
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
        context
            .local_flow()
            .first_unassigned_uses
            .iter()
            .filter_map(|&(symbol_id, reference_id)| {
                let first = tree.reference(reference_id);
                context.uninitialized_locals.is_field_write_prefix(first).then(|| {
                    Finding::new(
                        format!(
                            "field/index assignment on uninitialized `{}` (value is `nil`)",
                            tree.symbol(symbol_id).name
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
        let diags = run("\
local x
x.foo = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn flags_index_write_on_uninitialized() {
        let diags = run("\
local x
x[1] = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_when_initialized() {
        let diags = run("\
local x = {}
x.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_when_assigned_first() {
        let diags = run("\
local x
x = {}
x.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_read_before_field_write() {
        let diags = run("\
local x
print(x)
x.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("\
local _x
_x.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_forward_declaration_field_write_in_closure() {
        let diags = run("\
local M
local function init() M.x = 1 end
M = {}
init()");
        assert!(diags.is_empty(), "the closure runs after the assignment: {diags:?}");
    }
}
