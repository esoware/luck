use luck_ast::{Call, Expression, FunctionArgs};
use luck_semantic::SemanticAnalysis;

use crate::call::is_global_call;
use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnnecessaryAssert;

impl_rule!(
    UnnecessaryAssert,
    "unnecessary_assert",
    Suspicious,
    Warning,
    "`assert` on a constant value always passes or always errors"
);

impl NodeRule for UnnecessaryAssert {
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
    if !is_global_call(call, "assert", semantic) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = call.args else {
        return None;
    };
    let finding = match args.first()? {
        // 0 and "" are truthy in Lua, so every number and string passes.
        Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::InterpolatedString(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => {
            Finding::new("`assert` on a constant value always passes", call.span())
                .with_help("remove the `assert` or check a real condition")
        }
        Expression::False(_) | Expression::Nil(_) => {
            Finding::new("`assert` on a constant falsy value always errors", call.span())
                .with_help("use `error()` to raise unconditionally")
        }
        Expression::VarArg(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => return None,
    };
    Some(finding)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&UnnecessaryAssert, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_assert_true() {
        let diags = run("assert(true)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("always passes"), "{diags:?}");
    }

    #[test]
    fn flags_assert_zero() {
        // 0 is truthy in Lua.
        let diags = run("assert(0)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_assert_string() {
        let diags = run("assert(\"always\")");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_assert_table() {
        let diags = run("assert({})");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_assert_false() {
        let diags = run("assert(false, \"unreachable\")");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("always errors"), "{diags:?}");
    }

    #[test]
    fn flags_assert_nil() {
        let diags = run("assert(nil)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_assert_in_expr_position() {
        let diags = run("local ok = assert(true)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_variable_condition() {
        let diags = run("local x = 1\nassert(x)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_call_condition() {
        let diags = run("assert(next({}))");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_assert() {
        let diags = run("local assert = function() end\nassert(true)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_empty_assert() {
        let diags = run("assert()");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
