use luck_ast::{AstKind, AstType, Call, Expression, StringLiteral};
use luck_semantic::{AstNode, AstTypesBitset, Scoping};
use luck_token::BinOp;

use crate::binary::operator_pairs;
use crate::call::{CallArgs, is_global_call};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};
use crate::suggest::closest;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnknownType;

declare_lint_rule!(
    UnknownType,
    Correctness,
    Warning,
    "`type()` or `typeof()` result compared against an unknown type name"
);

impl Rule for UnknownType {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::BinaryOp]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::BinaryOp(run) = node.kind() else {
            return;
        };
        if !run.links.first().op.is_comparison() {
            return;
        }
        for pair in operator_pairs(run) {
            if matches!(pair.op, BinOp::Eq | BinOp::Ne)
                && let (Some(lhs), Some(rhs)) = (pair.lhs, pair.rhs)
                && let Some((function, literal)) = type_comparison(lhs, rhs, context.scoping())
            {
                check_literal(function, literal, context);
            }
        }
    }
}

/// The global whose result a comparison checks against a type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeFunction {
    Type,
    Typeof,
}

impl TypeFunction {
    /// The one `call` calls with one argument.
    fn of_call(call: Call<'_>, scoping: &Scoping) -> Option<Self> {
        let function = if is_global_call(call, "type", scoping) {
            Self::Type
        } else if is_global_call(call, "typeof", scoping) {
            Self::Typeof
        } else {
            return None;
        };
        (CallArgs::new(call.args).len() == 1).then_some(function)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Typeof => "typeof",
        }
    }
}

fn check_literal(
    function: TypeFunction,
    literal: &StringLiteral<'_>,
    context: &LintContext<'_, '_>,
) {
    let Some(name) = literal.as_str() else {
        return;
    };
    if PRIMITIVE_TYPE_NAMES.contains(&name) {
        return;
    }
    // Roblox `typeof` also returns datatype names (`"Instance"`,
    // `"Vector3"`) that aren't in this list, so only case typos of a
    // primitive are flagged there.
    let is_roblox_typeof =
        function == TypeFunction::Typeof && context.semantic.variant().is_roblox();
    let case_match =
        PRIMITIVE_TYPE_NAMES.iter().find(|primitive| primitive.eq_ignore_ascii_case(name));
    if is_roblox_typeof && case_match.is_none() {
        return;
    }
    let suggestion = case_match
        .copied()
        .or_else(|| closest(name, PRIMITIVE_TYPE_NAMES, |distance, _| distance <= 2));
    let mut finding = Finding::new(
        format!("`{name}` is not a value that `{}` can return", function.name()),
        literal.span,
    );
    if let Some(primitive) = suggestion {
        finding = finding.with_help(format!("did you mean `{primitive}`?"));
    }
    context.diagnostic(finding);
}

/// The type function one side of `lhs` and `rhs` calls, and the string
/// literal the other side is.
fn type_comparison<'ast>(
    lhs: &'ast Expression<'ast>,
    rhs: &'ast Expression<'ast>,
    scoping: &Scoping,
) -> Option<(TypeFunction, &'ast StringLiteral<'ast>)> {
    for (call_side, literal_side) in [(lhs, rhs), (rhs, lhs)] {
        let Expression::Suffixed(chain) = call_side else {
            continue;
        };
        let Some(function) =
            chain.last_call().and_then(|call| TypeFunction::of_call(call, scoping))
        else {
            continue;
        };
        let Expression::StringLiteral(literal) = literal_side else {
            continue;
        };
        return Some((function, literal));
    }
    None
}

const PRIMITIVE_TYPE_NAMES: [&str; 10] = [
    "nil", "boolean", "number", "string", "table", "function", "thread", "userdata", "buffer",
    "vector",
];

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        r#"local ok = type(x) == "nil""#,
        r#"local ok = type(x) == "boolean""#,
        r#"local ok = type(x) == "number""#,
        r#"local ok = type(x) == "string""#,
        r#"local ok = type(x) == "table""#,
        r#"local ok = type(x) == "function""#,
        r#"local ok = type(x) == "buffer""#,
        r#"local ok = typeof(x) == "vector""#,
        "\
local type = function() end
local ok = type(x) == \"strng\"",
        r#"local ok = name == "strng""#,
        "local ok = type(x) == expected",
    ];

    const FAIL: &[&str] = &[
        r#"local ok = type(x) == "strng""#,
        // Within 2 of `number`, which comes first, and within 1 of `buffer`.
        r#"local ok = type(x) == "nuffer""#,
        r#"local ok = "nll" == type(x)"#,
        r#"local ok = type(x) ~= "Number""#,
        r#"local ok = type"x" == "strng""#,
        r#"local ok = type{} == "strng""#,
        r#"local ok = typeof(x) == "tabel""#,
        r#"local ok = typeof(x) == "Instance""#,
    ];

    const ROBLOX_PASS: &[&str] = &[r#"local ok = typeof(x) == "Instance""#];

    const ROBLOX_FAIL: &[&str] = &[r#"local ok = typeof(x) == "Number""#];

    #[test]
    fn test() {
        Tester::new(RuleId::UnknownType, PASS, FAIL)
            .with_roblox(ROBLOX_PASS, ROBLOX_FAIL)
            .test_and_snapshot();
    }
}
