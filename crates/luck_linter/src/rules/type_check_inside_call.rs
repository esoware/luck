use luck_ast::Expression;
use luck_ast::visitor::Visitor;
use luck_semantic::SemanticAnalysis;
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

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
        let block = ctx.block;
        let semantic = ctx.semantic;
        let source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = TypeCheckChecker {
            source,
            semantic,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct TypeCheckChecker<'a> {
    source: &'a str,
    semantic: &'a SemanticAnalysis,
    diagnostics: Vec<LintDiagnostic>,
}

impl TypeCheckChecker<'_> {
    fn check_call(&mut self, call: &luck_ast::expr::FunctionCall) {
        let is_type_call = if let Expression::Var(var) = &call.callee {
            if let luck_ast::expr::Var::Name(token) = var.as_ref() {
                let name = &self.source[token.span.start as usize..token.span.end as usize];
                // Shadowed `type` is a user function.
                (name == "type" || name == "typeof")
                    && !self.semantic.resolves_to_local(name, token.span)
            } else {
                false
            }
        } else {
            false
        };

        if is_type_call && let luck_ast::expr::FunctionArgs::Parenthesized { args, .. } = &call.args
        {
            let single_arg = if args.len() == 1 { args.first() } else { None };

            if let Some(Expression::BinaryOp(binop)) = single_arg
                && matches!(binop.op.kind, TokenKind::EqualEqual | TokenKind::TildeEqual)
            {
                self.diagnostics.push(
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
}

impl Visitor for TypeCheckChecker<'_> {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt {
            self.check_call(&call_stmt.call);
        }
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.check_call(call);
        }
        self.walk_expression(expr);
    }
}
