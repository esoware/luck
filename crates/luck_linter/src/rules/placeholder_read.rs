use luck_ast::{Expression, Var};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags reads of `_`, which by convention names a value being thrown away.
///
/// A read usually means the author grabbed the wrong binding, or a nearer `_`
/// shadows the one they meant. Declared names and assignment targets aren't
/// expressions, so they never reach the hook. That includes Luau's `_ += 1`,
/// which does read `_`, but is rare and clearly deliberate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaceholderRead;

impl_rule!(
    PlaceholderRead,
    "placeholder_read",
    Suspicious,
    Warning,
    "use of `_` as a value; convention reserves it for ignored bindings"
);

impl NodeRule for PlaceholderRead {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::Var])
    }
    fn on_expr(&self, expr: &Expression, _context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        if let Expression::Var(Var::Name(token)) = expr
            && token.identifier() == Some("_")
        {
            findings.push(
                Finding::new(
                    "read of `_`, but convention is to use `_` only for ignored bindings",
                    token.span,
                )
                .with_help("rename the binding or pull the value into a meaningful local"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&PlaceholderRead, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_print_of_underscore() {
        let diags = run("print(_)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_local_declaration() {
        let diags = run("local _ = f()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_assignment_target() {
        let diags = run("_ = f()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_prefix_and_key_of_assignment_target() {
        let diags = run("_[_] = 1\n_.x = 2");
        assert_eq!(diags.len(), 3, "{diags:?}");
    }

    #[test]
    fn ignores_compound_assignment_target() {
        let diags = run_rule(&PlaceholderRead, "_ += 1", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_read_inside_generic_for_body() {
        let diags = run("for _, v in pairs(t) do print(_) end");
        assert_eq!(diags.len(), 1, "binding should not fire, body read should: {diags:?}");
    }

    #[test]
    fn ignores_numeric_for_binding() {
        let diags = run("for _ = 1, 10 do end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_function_parameter() {
        let diags = run("local function f(_) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_read_inside_function_body() {
        let diags = run("local function f(_) return _ end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }
}
