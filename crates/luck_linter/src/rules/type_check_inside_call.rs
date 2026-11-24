use luck_ast::{AstKind, AstType, Call, Expression, FunctionArgs, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::SemanticAnalysis;
use luck_token::BinOp;

use crate::call::{calls_in, is_global_call};
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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (_, call) in calls_in(chain) {
            if let Some(finding) = check_call(call, context.semantic) {
                context.diagnostic(finding);
            }
        }
    }
}

fn check_call(call: Call<'_>, semantic: &SemanticAnalysis<'_>) -> Option<Finding> {
    if !is_global_call(call, "type", semantic) && !is_global_call(call, "typeof", semantic) {
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
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::TypeCheckInsideCall, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_equality_inside_type_call() {
        let diags = run("if type(x == \"string\") then end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_inequality_inside_typeof_call() {
        let diags = run("local ok = typeof(x ~= \"Instance\")");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_method_call_on_a_table_named_type() {
        let source = "\
type = {}
function type:f(v) return v end
type:f(1 == 2)";
        let diags = run(source);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_method_call_on_a_table_named_typeof() {
        let diags = run("typeof:f(a == b)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_comparison_outside_call() {
        let diags = run("local ok = type(x) == \"string\"");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_comparison_among_several_args() {
        let diags = run("type(x == 1, y)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_type() {
        let diags = run("\
local function type(v) return v end
local ok = type(x == 1)");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
