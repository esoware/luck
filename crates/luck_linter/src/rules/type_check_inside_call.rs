use luck_ast::{AstKind, AstType, Call, Expression, FunctionArgs, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset, Scoping};
use luck_token::BinOp;

use crate::call::is_global_call;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags `type(x == "string")` written for `type(x) == "string"`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TypeCheckInsideCall;

declare_lint_rule!(
    TypeCheckInsideCall,
    Correctness,
    Error,
    "type comparison is inside the call instead of outside"
);

impl Rule for TypeCheckInsideCall {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (_, call) in chain.calls() {
            if let Some(finding) = check_call(call, context.scoping()) {
                context.diagnostic(finding);
            }
        }
    }
}

fn check_call(call: Call<'_>, scoping: &Scoping) -> Option<Finding> {
    if !is_global_call(call, "type", scoping) && !is_global_call(call, "typeof", scoping) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = call.args else {
        return None;
    };
    let [Expression::BinaryOp(binary_op)] = &**args else {
        return None;
    };
    // A run of comparisons groups from the left, so its last one is
    // outermost.
    matches!(binary_op.links.last().op, BinOp::Eq | BinOp::Ne).then(|| {
        Finding::new(
            "comparison is inside the `type()` call, did you mean `type(x) == \"string\"`?",
            call.span(),
        )
        .with_help("move the comparison outside: `type(x) == \"string\"`")
    })
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
type = {}
function type:f(v) return v end
type:f(1 == 2)",
        "typeof:f(a == b)",
        r#"local ok = type(x) == "string""#,
        "type(x == 1, y)",
        "\
local function type(v) return v end
local ok = type(x == 1)",
    ];

    const FAIL: &[&str] =
        &[r#"if type(x == "string") then end"#, r#"local ok = typeof(x ~= "Instance")"#];

    #[test]
    fn test() {
        Tester::new(RuleId::TypeCheckInsideCall, PASS, FAIL).test_and_snapshot();
    }
}
