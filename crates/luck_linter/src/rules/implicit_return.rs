use luck_ast::{Expression, Statement};
use luck_semantic::SemanticAnalysis;

use crate::cfg::{Exit, FlowGraph};
use crate::diagnostic::Finding;
use crate::function_node::FunctionNode;
use crate::node_type::NodeTypeSet;
use crate::rule::{LintContext, NodeRule, impl_rule};

/// A function that never returns a value is a procedure and isn't flagged,
/// so a bare `return` used as a guard clause doesn't count.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ImplicitReturn;

// "Return a value or nil" is idiomatic Lua, and neither Selene nor
// Luacheck flags it, so this stays off by default.
impl_rule!(
    ImplicitReturn,
    "implicit_return",
    Suspicious,
    Warning,
    "function returns on some paths but falls through on others"
);

impl NodeRule for ImplicitReturn {
    fn node_types(&self) -> NodeTypeSet {
        FunctionNode::NODE_TYPES
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        findings.extend(
            FunctionNode::of_stmt(stmt)
                .and_then(|function| check_function(function, context.semantic)),
        );
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        findings.extend(
            FunctionNode::of_expr(expr)
                .and_then(|function| check_function(function, context.semantic)),
        );
    }
}

fn check_function(function: FunctionNode<'_>, semantic: &SemanticAnalysis) -> Option<Finding> {
    let graph = FlowGraph::of_block(&function.body.block, semantic);
    (graph.reaches(Exit::ReturnValue) && graph.reaches(Exit::End)).then(|| {
        Finding::new("function returns on some paths but falls through on others", function.span)
            .with_help("add a `return` on the fallthrough path to make the implicit `nil` explicit")
    })
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&ImplicitReturn, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_if_only_branch_returns() {
        let diags = run("function f(x) if x then return 1 end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("falls through"));
    }

    #[test]
    fn ignores_all_branches_return() {
        let diags = run("function f(x) if x then return 1 else return 2 end end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_no_return_anywhere() {
        let diags = run("function f() print() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_explicit_bare_return() {
        // A bare `return` at the end is explicit, so nothing falls through.
        let diags = run("function f() return end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_local_function_with_mixed_paths() {
        let diags = run("local function f(x) if x then return 1 end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_anonymous_function_with_mixed_paths() {
        let diags = run("local f = function(x) if x then return 1 end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_global_function_with_mixed_paths() {
        let diags = run_rule(
            &ImplicitReturn,
            "global function f(x) if x then return 1 end end",
            LuaVersion::Lua55,
        );
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_type_function_with_mixed_paths() {
        let source = "type function t1(a) if a then return a end end";
        let diags = run_rule(&ImplicitReturn, source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_type_alias() {
        let diags = run_rule(&ImplicitReturn, "type T = number", LuaVersion::Luau);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_only_the_nested_function_with_mixed_paths() {
        // The outer function never returns, so only the inner one fires.
        let source = "function f() local function g(x) if x then return 1 end end end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_elseif_chain_without_else() {
        let diags = run("function f(x, y) if x then return 1 elseif y then return 2 end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_return_only_inside_for_loop() {
        // The loop body may never run, so the function can fall through.
        let diags = run("function f() for i = 1, 10 do return i end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_while_true_with_return() {
        // `while true do ... return end` always returns.
        let diags = run("function f() while true do return 1 end end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_fallthrough_after_while_true_with_conditional_break() {
        let diags = run(
            "local function f(c, d) if c then return 1 end while true do if d then break end end end",
        );
        assert_eq!(diags.len(), 1, "the `break` path falls off the end: {diags:?}");
    }

    #[test]
    fn ignores_return_after_repeat_continue_until_true() {
        let source = "\
local function f(x)
 if x then return 1 end
 repeat continue until true
 return 2
end";
        let diags = run_rule(&ImplicitReturn, source, LuaVersion::Luau);
        assert!(diags.is_empty(), "`continue` ends the repeat, so `return 2` runs: {diags:?}");
    }

    #[test]
    fn ignores_while_one_infinite_loop() {
        let diags = run("local function f(x) while 1 do if x then return 1 end end end");
        assert!(diags.is_empty(), "`1` is always truthy, so nothing falls through: {diags:?}");
    }

    #[test]
    fn ignores_guard_clause_bare_return() {
        let diags = run("local function f(x) if not x then return end print(x) end");
        assert!(diags.is_empty(), "no path returns a value, so no `nil` is implicit: {diags:?}");
    }

    #[test]
    fn flags_value_return_beside_a_guard_clause() {
        let diags = run("local function f(x, y) if x then return end if y then return 1 end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }
}
