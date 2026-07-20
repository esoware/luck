use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall, Var};
use luck_token::TokenKind;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct UnnecessaryAssert;

impl Rule for UnnecessaryAssert {
    fn name(&self) -> &'static str {
        "unnecessary_assert"
    }

    fn category(&self) -> Category {
        Category::Suspicious
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "assert on a constant value always passes or always errors"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for UnnecessaryAssert {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::stmt::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::stmt::Statement::FunctionCall(call_stmt) = stmt {
            check_call(&call_stmt.call, ctx, out);
        }
    }

    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::FunctionCall(call) = expr {
            check_call(call, ctx, out);
        }
    }
}

fn check_call(call: &FunctionCall, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
    if !is_global_assert(call, ctx) {
        return;
    }
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return;
    };
    let Some(condition) = args.iter().next() else {
        return;
    };
    match condition {
        // 0 and "" are truthy in Lua, so every number and string passes.
        Expression::True(_)
        | Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::InterpolatedString(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => {
            out.push(
                LintDiagnostic::new(
                    "unnecessary_assert",
                    "assert on a constant value always passes",
                    call.span,
                )
                .with_help("remove the assert or check a real condition".to_string()),
            );
        }
        Expression::False(_) | Expression::Nil(_) => {
            out.push(
                LintDiagnostic::new(
                    "unnecessary_assert",
                    "assert on a constant falsy value always errors",
                    call.span,
                )
                .with_help("use error() to raise unconditionally".to_string()),
            );
        }
        _ => {}
    }
}

fn is_global_assert(call: &FunctionCall, ctx: &LintContext) -> bool {
    if call.method.is_some() {
        return false;
    }
    let Expression::Var(var) = &call.callee else {
        return false;
    };
    let Var::Name(token) = var else {
        return false;
    };
    let TokenKind::Identifier(name) = &token.kind else {
        return false;
    };
    name.as_str() == "assert" && !ctx.semantic.resolves_to_local(name.as_str(), token.span)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::UnnecessaryAssert;
    use crate::diagnostic::LintDiagnostic;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&UnnecessaryAssert, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_assert_true() {
        let diags = run("assert(true)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("always passes"), "{diags:?}");
    }

    #[test]
    fn flags_assert_zero() {
        // 0 is truthy in Lua.
        let diags = run("assert(0)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_assert_string() {
        let diags = run("assert(\"always\")");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_assert_table() {
        let diags = run("assert({})");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_assert_false() {
        let diags = run("assert(false, \"unreachable\")");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("always errors"), "{diags:?}");
    }

    #[test]
    fn flags_assert_nil() {
        let diags = run("assert(nil)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_assert_in_expression_position() {
        let diags = run("local ok = assert(true)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_variable_condition() {
        let diags = run("local x = 1\nassert(x)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_call_condition() {
        let diags = run("assert(next({}))");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_assert() {
        let diags = run("local assert = function() end\nassert(true)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_empty_assert() {
        let diags = run("assert()");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
