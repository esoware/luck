use luck_ast::{Call, Expression, FunctionArgs};
use luck_semantic::SemanticAnalysis;
use luck_token::BinOp;

use crate::call::is_global_call;
use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

/// Flags `type(x == "string")` written for `type(x) == "string"`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypeCheckInsideCall;

impl_rule!(
    TypeCheckInsideCall,
    "type_check_inside_call",
    Correctness,
    Error,
    "type comparison is inside the call instead of outside"
);

impl NodeRule for TypeCheckInsideCall {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr])
    }
    fn on_call(
        &self,
        call: Call<'_>,
        _position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        findings.extend(check_call(call, context.semantic));
    }
}

fn check_call(call: Call<'_>, semantic: &SemanticAnalysis) -> Option<Finding> {
    if !is_global_call(call, "type", semantic) && !is_global_call(call, "typeof", semantic) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = call.args else {
        return None;
    };
    let [Expression::BinaryOp(binary_op)] = args.items.as_slice() else {
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

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&TypeCheckInsideCall, source, LuaVersion::Lua54)
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
        let source = "type = {}\nfunction type:f(v) return v end\ntype:f(1 == 2)";
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
        let diags = run("local function type(v) return v end\nlocal ok = type(x == 1)");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
