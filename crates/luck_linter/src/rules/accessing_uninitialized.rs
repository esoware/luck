use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Luacheck 321: a local declared without a value is read where no assignment
/// can have reached it.
///
/// A name counts when its `local` gives it no value, as in `local x` or the
/// `b` of `local a, b = 1`. `local x = nil` is initialized, and
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
        context
            .local_flow()
            .first_unassigned_uses
            .iter()
            .filter_map(|&(symbol_id, reference_id)| {
                let first = tree.reference(reference_id);
                (!context.uninitialized_locals.is_field_write_prefix(first)).then(|| {
                    Finding::new(
                        format!(
                            "`{}` is read before any value is assigned, so it is `nil`",
                            tree.symbol(symbol_id).name
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
        let diags = run("\
local x
print(x)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_explicit_nil_init() {
        // `redundant_nil_init` may flag this, but it counts as initialized.
        let diags = run("\
local x = nil
print(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_write_then_read() {
        let diags = run("\
local x
x = 1
print(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_write_in_branch_before_read() {
        let diags = run("\
local x
if c then x = 1 end
print(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_field_write_as_the_first_reference() {
        let diags = run("\
local x
x.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_index_write_as_the_first_reference() {
        let diags = run("\
local y
y[1] = 2");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_field_read_through_a_nested_write_target() {
        // `x.y.z = 1` reads `x.y` first, which isn't a write through `x`.
        let diags = run("\
local x
x.y.z = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("\
local _x
print(_x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_forward_declaration_read_in_closure() {
        let diags = run("\
local helper
local function main() return helper() end
helper = function() return 1 end
return main()");
        assert!(diags.is_empty(), "the closure runs after the assignment: {diags:?}");
    }

    #[test]
    fn ignores_read_of_value_written_in_previous_iteration() {
        let diags = run("\
local last
for _, v in ipairs({1}) do if last then print(last) end last = v end");
        assert!(diags.is_empty(), "later iterations read the written value: {diags:?}");
    }

    #[test]
    fn ignores_read_after_backward_goto() {
        let diags = run("\
local x
goto set
::use::
print(x)
do return end
::set::
x = 1
goto use");
        assert!(diags.is_empty(), "the assignment runs before the read: {diags:?}");
    }

    #[test]
    fn flags_read_in_own_first_assignment() {
        let diags = run("\
local x
x = x + 1
return x");
        assert_eq!(diags.len(), 1, "the right-hand side reads `nil`: {diags:?}");
    }

    #[test]
    fn flags_read_in_the_branch_that_skips_the_write() {
        let diags = run("\
local x
if c then x = 1 else print(x) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_read_before_the_closure_that_writes_it() {
        let diags = run("\
local x
print(x)
local function set() x = 1 end
set()");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_read_after_a_closure_that_writes_it() {
        let diags = run("\
local x
local function set() x = 1 end
set()
print(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_read_of_a_name_the_initializer_list_leaves_without_a_value() {
        let diags = run("\
local a, b = 1
print(a, b)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`b`"), "got: {diags:?}");
    }

    #[test]
    fn ignores_name_a_trailing_call_can_fill() {
        let diags = run("\
local a, b = f()
print(a, b)");
        assert!(diags.is_empty(), "the call can fill `b`: {diags:?}");
    }

    #[test]
    fn ignores_read_in_until_after_a_write_in_the_body() {
        let diags = run("\
repeat local x
if c then x = 1 end until x");
        assert!(diags.is_empty(), "the `until` sees the body's locals: {diags:?}");
    }

    #[test]
    fn flags_read_after_the_loop_redeclares_the_local() {
        let diags = run("\
for _ = 1, 3 do local x
print(x)
x = 1 end");
        assert_eq!(diags.len(), 1, "a fresh `x` starts each iteration: {diags:?}");
    }
}
