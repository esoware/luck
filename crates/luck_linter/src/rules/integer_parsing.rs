use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags a numeral spelled as an integer that doesn't read as that integer,
/// since every number is a double and an integer above 2^53 can round.
///
/// The parser read it as Luau does, and marked it so. A float spelling counts
/// as exact, since nobody writes one expecting an exact integer.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct IntegerParsing;

declare_lint_rule!(
    IntegerParsing,
    Correctness,
    Warning,
    "integer literal a double can't represent exactly"
);

impl Rule for IntegerParsing {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::NumberLiteral]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        if let AstKind::NumberLiteral(literal) = node.kind()
            && let Some(raw) = literal.raw
            && !literal.is_exact
        {
            context.diagnostic(
                Finding::new(
                    format!(
                        "integer literal `{raw}` exceeds 2^53, so it loses precision when stored as a double"
                    ),
                    literal.span,
                )
                .with_help("use a smaller value or store as a string"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "local x = 0xFF",
        "local x = 100",
        "local x = 1.5",
        // 1000 is exact in a double.
        "local x = 1_000",
        // 5 is exact in a double.
        "local x = 0b101",
        // 2^53 + 2 is exact in a double.
        "local x = 9007199254740994",
    ];

    const FAIL: &[&str] = &[
        "local x = 0xFFFFFFFFFFFFFFFFFF",
        "local x = 9007199254740993",
        "local x = 99999999999999999999",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::IntegerParsing, PASS, FAIL).test_and_snapshot();
    }
}
