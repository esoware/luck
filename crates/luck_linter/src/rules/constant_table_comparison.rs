use luck_ast::Expression;
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct ConstantTableComparison;

impl Rule for ConstantTableComparison {
    fn name(&self) -> &'static str {
        "constant_table_comparison"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "comparing with a table literal always fails (tables compare by identity)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for ConstantTableComparison {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::BinaryOp]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, _ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::BinaryOp(binop) = expr
            && matches!(binop.op.kind, TokenKind::EqualEqual | TokenKind::TildeEqual)
            && (matches!(&binop.left, Expression::TableConstructor(_))
                || matches!(&binop.right, Expression::TableConstructor(_)))
        {
            out.push(
                LintDiagnostic::new(
                    "constant_table_comparison",
                    "comparing with a table literal always fails (tables compare by identity)"
                        .to_string(),
                    binop.span,
                )
                .with_help("compare table contents instead".to_string()),
            );
        }
    }
}
