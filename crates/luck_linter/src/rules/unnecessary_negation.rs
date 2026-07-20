use luck_ast::expr::Expression;
use luck_token::Span;
use luck_token::{BinOp, UnOp};

use crate::diagnostic::{Category, Fix, LintDiagnostic, Severity, TextEdit};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct UnnecessaryNegation;

impl Rule for UnnecessaryNegation {
    fn name(&self) -> &'static str {
        "unnecessary_negation"
    }

    fn category(&self) -> Category {
        Category::Style
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "negated comparison that could use the opposite operator"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn flipped_operator(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Eq => Some("~="),
        BinOp::Ne => Some("=="),
        BinOp::Lt => Some(">="),
        BinOp::Le => Some(">"),
        BinOp::Gt => Some("<="),
        BinOp::Ge => Some("<"),
        _ => None,
    }
}

fn slice(source: &str, span: Span) -> Option<&str> {
    source.get(span.start as usize..span.end as usize)
}

impl NodeRule for UnnecessaryNegation {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::UnaryOp]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        let Expression::UnaryOp(unary) = expr else {
            return;
        };
        if unary.op != UnOp::Not {
            return;
        }
        let Expression::Parenthesized(paren) = &unary.operand else {
            return;
        };
        let Expression::BinaryOp(comparison) = &paren.expr else {
            return;
        };
        let Some(flipped) = flipped_operator(comparison.op) else {
            return;
        };
        let (Some(lhs), Some(rhs)) = (
            slice(ctx.source, comparison.left.span()),
            slice(ctx.source, comparison.right.span()),
        ) else {
            return;
        };
        // Only ==/~= get an autofix: that inversion is exact (~= is defined
        // as the negation of ==, sharing __eq), while relational flips
        // differ for NaN and __lt/__le metamethods.
        let is_equality = matches!(comparison.op, BinOp::Eq | BinOp::Ne);
        let fix = is_equality.then(|| Fix {
            description: "invert the comparison operator".to_string(),
            // The parens stay: with no parent context here, a bare
            // comparison could regroup under a tighter-binding neighbor
            // (`..`, arithmetic, or an adjacent comparison).
            edits: vec![TextEdit {
                span: unary.span,
                replacement: format!("({lhs} {flipped} {rhs})"),
            }],
        });
        out.push(
            LintDiagnostic::new(
                "unnecessary_negation",
                format!("negated comparison; consider `{lhs} {flipped} {rhs}`"),
                unary.span,
            )
            .with_fix_opt(fix),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&UnnecessaryNegation, source, LuaVersion::Lua54)
    }

    fn apply(source: &str, diag: &LintDiagnostic) -> String {
        let fix = diag.fix.as_ref().expect("fix");
        let edit = &fix.edits[0];
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..edit.span.start as usize]);
        out.push_str(&edit.replacement);
        out.push_str(&source[edit.span.end as usize..]);
        out
    }

    #[test]
    fn flags_negated_equality() {
        let diags = run("local r = not (a == b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("a ~= b"), "{diags:?}");
        assert!(diags[0].fix.is_some(), "{diags:?}");
    }

    #[test]
    fn flags_negated_inequality() {
        let diags = run("local r = not (a ~= b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_some(), "{diags:?}");
    }

    #[test]
    fn flags_negated_relational_without_fix() {
        let diags = run("local r = not (a < b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("a >= b"), "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn flags_negated_greater_equal_without_fix() {
        let diags = run("local r = not (a >= b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("a < b"), "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn ignores_negated_and() {
        let diags = run("local r = not (a and b)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_negated_arithmetic() {
        let diags = run("local r = not (a + b)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_unparenthesized_negation() {
        // `not a == b` parses as `(not a) == b`; comparison_precedence
        // owns that shape.
        let diags = run("local r = not a == b");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn fix_inverts_equality_and_reparses() {
        let source = "local r = not (a == b)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "local r = (a ~= b)");
        let parse = luck_parser::parse(&fixed, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
    }

    #[test]
    fn fix_inverts_inequality_and_reparses() {
        let source = "local r = not (x.y ~= f(1))";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "local r = (x.y == f(1))");
        let parse = luck_parser::parse(&fixed, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
    }
}
