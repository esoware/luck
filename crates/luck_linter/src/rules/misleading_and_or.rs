use luck_ast::Expression;
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

/// `(cond and a) or b` is the canonical Lua ternary idiom, but it
/// collapses to `b` whenever `a` is `false` or `nil`. Writing
/// `(cond and false) or alt` or `(cond and nil) or alt` therefore always
/// evaluates to `alt`, defeating the conditional entirely.
pub struct MisleadingAndOr;

impl Rule for MisleadingAndOr {
    fn name(&self) -> &'static str {
        "misleading_and_or"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "and/or ternary with a falsy middle operand always returns the right-hand value"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

/// Look through an explicit parenthesized wrapper. The pattern reads
/// most naturally with parens so users write it both ways.
fn unparen(expr: &Expression) -> &Expression {
    if let Expression::Parenthesized(paren) = expr {
        return &paren.expr;
    }
    expr
}

fn is_falsy_literal(expr: &Expression) -> bool {
    matches!(unparen(expr), Expression::Nil(_) | Expression::False(_))
}

impl NodeRule for MisleadingAndOr {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::BinaryOp]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, _ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::BinaryOp(outer) = expr
            && matches!(outer.op.kind, TokenKind::Or)
        {
            let lhs = unparen(&outer.left);
            if let Expression::BinaryOp(inner) = lhs
                && matches!(inner.op.kind, TokenKind::And)
                && is_falsy_literal(&inner.right)
            {
                out.push(
                    LintDiagnostic::new(
                        "misleading_and_or",
                        "`(x and FALSY) or y` always evaluates to `y`; the ternary collapses"
                            .to_string(),
                        outer.span,
                    )
                    .with_help(
                        "use an `if` statement or a Luau `if-expression` for a real ternary"
                            .to_string(),
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&MisleadingAndOr, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_nil_middle() {
        let diags = run("local r = (x and nil) or y");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_false_middle() {
        let diags = run("local r = (x and false) or y");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_unparenthesized_form() {
        // Lua precedence: `and` binds tighter than `or`, so this is the
        // same shape with no parens.
        let diags = run("local r = x and nil or y");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_zero_middle() {
        // `0` is truthy in Lua, so the ternary works as intended.
        let diags = run("local r = (x and 0) or y");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_normal_ternary() {
        let diags = run("local r = (x and y) or z");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_or_without_and() {
        let diags = run("local r = x or y");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
