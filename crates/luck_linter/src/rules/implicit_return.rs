use luck_ast::{Expression, Statement};

use crate::cfg::analyze_full_block;
use crate::diagnostic::{Category, Finding, Severity};
use crate::function_node::FunctionNode;
use crate::node_type::NodeTypeSet;
use crate::rule::{LintContext, NodeRule, Rule};

/// Flags functions that return on some paths but fall off the end on
/// others.
///
/// A function with no `return` at all is a procedure and isn't flagged.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ImplicitReturn;

impl Rule for ImplicitReturn {
    fn name(&self) -> &'static str {
        "implicit_return"
    }
    fn category(&self) -> Category {
        // "Return a value or nil" is idiomatic Lua, and neither Selene nor
        // Luacheck flags it, so this stays off by default.
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "function returns on some paths but falls through on others"
    }
}

impl NodeRule for ImplicitReturn {
    fn node_types(&self) -> NodeTypeSet {
        FunctionNode::NODE_TYPES
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        findings.extend(FunctionNode::of_statement(stmt).and_then(check_function));
    }
    fn on_expression(
        &self,
        expr: &Expression,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        findings.extend(FunctionNode::of_expression(expr).and_then(check_function));
    }
}

fn check_function(function: FunctionNode<'_>) -> Option<Finding> {
    let summary = analyze_full_block(&function.body.block);
    (summary.can_return && !summary.always_returns()).then(|| {
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
}
