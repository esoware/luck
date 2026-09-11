use luck_ast::Expression;
use luck_ast::shared::FunctionBody;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;
use luck_token::Span;

use crate::cfg::analyze_full_block;
use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Detects functions where some control paths return a value while
/// others fall off the end (an implicit `nil` return). A function with
/// no `return` anywhere is an imperative procedure and stays unflagged.
/// Only mixed return/fallthrough functions fire.
pub struct ImplicitReturn;

impl Rule for ImplicitReturn {
    fn name(&self) -> &'static str {
        "implicit_return"
    }
    fn category(&self) -> Category {
        // Mixed implicit and explicit returns are idiomatic Lua ("return
        // value or nil"), and neither Selene nor Luacheck flags them, so
        // this rule is not default-on.
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "function returns a value on some paths but falls through on others"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let mut checker = ImplicitReturnChecker {
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct ImplicitReturnChecker {
    diagnostics: Vec<LintDiagnostic>,
}

impl ImplicitReturnChecker {
    fn check_function_body(&mut self, body: &FunctionBody, span: Span) {
        let summary = analyze_full_block(&body.block);
        // A function that never returns anywhere is a procedure, and
        // leaving its end open is the intended pattern.
        if !summary.may_return {
            return;
        }
        if summary.always_returns {
            return;
        }
        self.diagnostics.push(
            LintDiagnostic::new(
                "implicit_return",
                "function returns on some paths but falls through on others".to_string(),
                span,
            )
            .with_help(
                "add an explicit `return` on the fallthrough path to make the implicit nil clear"
                    .to_string(),
            ),
        );
    }
}

impl<'ast> Visitor<'ast> for ImplicitReturnChecker {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        // Each function is its own control-flow unit, so scan the body
        // before recursing into nested functions.
        match stmt {
            Statement::FunctionDecl(decl) => {
                self.check_function_body(&decl.body, decl.span);
            }
            Statement::LocalFunction(local) => {
                self.check_function_body(&local.body, local.span);
            }
            Statement::GlobalFunction(global) => {
                self.check_function_body(&global.body, global.span);
            }
            _ => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionDef(func_def) = expr {
            self.check_function_body(&func_def.body, func_def.span);
        }
        self.walk_expression(expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&ImplicitReturn, source, LuaVersion::Lua54)
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
        // A bare `return` at the end is explicit, so the function always
        // returns and never falls through.
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
    fn nested_functions_scanned_independently() {
        // The outer function never returns, so only the inner one fires.
        let source = "function f() local function g(x) if x then return 1 end end end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(!diags.is_empty());
    }

    #[test]
    fn return_in_elseif_chain_missing_else() {
        let diags = run("function f(x, y) if x then return 1 elseif y then return 2 end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn return_in_for_loop_only() {
        // for-loop body may not execute, so the function falls through
        // even though `may_return` is true.
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
