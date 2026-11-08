use luck_ast::{Expression, FunctionBody, NodeType, NodeTypeSet, Statement};
use luck_token::Span;

use crate::cfg::analyze_full_block;
use crate::diagnostic::{Category, LintDiagnostic, Severity};
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
        NodeTypeSet::from_types(&[
            NodeType::FunctionDecl,
            NodeType::LocalFunction,
            NodeType::GlobalFunction,
            NodeType::FunctionDef,
        ])
    }

    fn on_statement(
        &self,
        stmt: &Statement,
        _context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        let (body, span) = match stmt {
            Statement::FunctionDecl(decl) => (&decl.body, decl.span),
            Statement::LocalFunction(local_function) => (&local_function.body, local_function.span),
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                (&global_function.body, global_function.span)
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => return,
        };
        diags.extend(check_function(body, span));
    }

    fn on_expression(
        &self,
        expr: &Expression,
        _context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        if let Expression::FunctionDef(function) = expr {
            diags.extend(check_function(&function.body, function.span));
        }
    }
}

fn check_function(body: &FunctionBody, span: Span) -> Option<LintDiagnostic> {
    let summary = analyze_full_block(&body.block);
    (summary.can_return && !summary.always_returns()).then(|| {
        LintDiagnostic::new(
            "implicit_return",
            "function returns on some paths but falls through on others",
            span,
        )
        .with_help("add a `return` on the fallthrough path to make the implicit `nil` explicit")
    })
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
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
