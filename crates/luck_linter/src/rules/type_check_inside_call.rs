use luck_ast::Expression;
use luck_ast::node::{AstTypesBitset, NodeType};
use luck_token::BinOp;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

/// Detects `type(x == "string")` instead of `type(x) == "string"`.
pub struct TypeCheckInsideCall;

impl Rule for TypeCheckInsideCall {
    fn name(&self) -> &'static str {
        "type_check_inside_call"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "type comparison is inside the call instead of outside"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn check_call(
    call: &luck_ast::expr::FunctionCall,
    ctx: &LintContext,
    out: &mut Vec<LintDiagnostic>,
) {
    let is_type_call = if let Expression::Var(luck_ast::expr::Var::Name(token)) = &call.callee
        && let luck_token::TokenKind::Identifier(name) = &token.kind
    {
        // Shadowed `type` is a user function.
        (name == "type" || name == "typeof") && !ctx.semantic.resolves_to_local(name, token.span)
    } else {
        false
    };

    if is_type_call && let luck_ast::expr::FunctionArgs::Parenthesized { args, .. } = &call.args {
        let single_arg = if args.len() == 1 { args.first() } else { None };

        if let Some(Expression::BinaryOp(binop)) = single_arg
            && matches!(binop.op, BinOp::Eq | BinOp::Ne)
        {
            out.push(
                LintDiagnostic::new(
                    "type_check_inside_call",
                    "comparison is inside type() call; did you mean `type(x) == \"string\"`?"
                        .to_string(),
                    call.span,
                )
                .with_help("move the comparison outside: `type(x) == \"string\"`".to_string()),
            );
        }
    }
}

impl NodeRule for TypeCheckInsideCall {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt {
            check_call(&call_stmt.call, ctx, out);
        }
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::FunctionCall(call) = expr {
            check_call(call, ctx, out);
        }
    }
}
