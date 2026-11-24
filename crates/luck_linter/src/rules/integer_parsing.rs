use luck_ast::{AstKind, AstType, NumberLiteral};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::LuaVersion;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Lua 5.1, 5.2, and Luau store every number as a double, so integers above
/// 2^53 can round.
///
/// Lua 5.3+ has 64-bit integers: a decimal literal above 2^63-1 becomes a
/// float, and a hex literal above 2^64-1 wraps around.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct IntegerParsing;

declare_lint_rule!(
    IntegerParsing,
    Correctness,
    Warning,
    "integer literal the target Lua version can't represent exactly"
);

impl Rule for IntegerParsing {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::NumberLiteral]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        if let AstKind::NumberLiteral(literal) = node.kind()
            && let Some(finding) = check_number(literal, context.semantic.version())
        {
            context.diagnostic(finding);
        }
    }
}

/// Flags a numeral spelled as an integer that doesn't read as that integer.
///
/// The parser read it as its reference does, and marked it so. A float
/// spelling counts as exact, since nobody writes one expecting an exact
/// integer.
fn check_number(literal: &NumberLiteral<'_>, version: LuaVersion) -> Option<Finding> {
    let raw = literal.raw?;
    if literal.is_exact {
        return None;
    }
    let (message, help) = if !version.has_integer_subtype() {
        (
            format!(
                "integer literal `{raw}` exceeds 2^53, so it loses precision when stored as a double"
            ),
            "use a smaller value or store as a string",
        )
    } else if !raw.starts_with("0x") && !raw.starts_with("0X") {
        (
            format!(
                "decimal integer `{raw}` exceeds 2^63-1, so it will be parsed as a float and lose precision"
            ),
            "use a hex literal or split into smaller values",
        )
    } else {
        (
            format!(
                "hex literal `{raw}` exceeds the 64-bit integer range, so it wraps around to a different value"
            ),
            "use a smaller value or split across two values",
        )
    };
    Some(Finding::new(message, literal.span).with_help(help))
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("local x = 0xFF", LuaTarget::Lua53),
        ("local x = 100", LuaTarget::Lua54),
        ("local x = 1.5", LuaTarget::Lua54),
        // 1000 is exact in a double.
        ("local x = 1_000", LuaTarget::Luau),
        // 5 is exact in a double.
        ("local x = 0b101", LuaTarget::Luau),
        // 2^53 + 2 is exact in a double.
        ("local x = 9007199254740994", LuaTarget::Lua51),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("local x = 0xFFFFFFFFFFFFFFFFFF", LuaTarget::Lua53),
        ("local x = 9007199254740993", LuaTarget::Lua51),
        ("local x = 99999999999999999999", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::IntegerParsing, PASS, FAIL).test_and_snapshot();
    }
}
