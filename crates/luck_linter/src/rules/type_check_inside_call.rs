use luck_ast::{AstKind, AstType, Call, Expression, FunctionArgs, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::SemanticAnalysis;
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
            if let Some(finding) = check_call(call, context.semantic) {
                context.diagnostic(finding);
            }
        }
    }
}

fn check_call(call: Call<'_>, semantic: &SemanticAnalysis<'_>) -> Option<Finding> {
    let scoping = semantic.scoping();
    let is_type_call = is_global_call(call, "type", scoping)
        || (semantic.version().is_luau() && is_global_call(call, "typeof", scoping));
    if !is_type_call {
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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        (
            "\
type = {}
function type:f(v) return v end
type:f(1 == 2)",
            LuaTarget::Lua54,
        ),
        ("typeof:f(a == b)", LuaTarget::Luau),
        (r#"local ok = type(x) == "string""#, LuaTarget::Lua54),
        ("type(x == 1, y)", LuaTarget::Lua54),
        (
            "\
local function type(v) return v end
local ok = type(x == 1)",
            LuaTarget::Lua54,
        ),
        // `typeof` isn't a Lua 5.4 builtin, so this one is the program's.
        ("local ok = typeof(x ~= nil)", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (r#"if type(x == "string") then end"#, LuaTarget::Lua54),
        (r#"local ok = typeof(x ~= "Instance")"#, LuaTarget::Luau),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::TypeCheckInsideCall, PASS, FAIL).test_and_snapshot();
    }
}
