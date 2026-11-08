use luck_ast::{Expression, FunctionArgs, FunctionCall, NodeType, NodeTypeSet, Statement};

use crate::call::is_global_call;
use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnnecessaryAssert;

impl Rule for UnnecessaryAssert {
    fn name(&self) -> &'static str {
        "unnecessary_assert"
    }

    fn category(&self) -> Category {
        Category::Suspicious
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "`assert` on a constant value always passes or always errors"
    }
}

impl NodeRule for UnnecessaryAssert {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        if let Statement::FunctionCall(call) = stmt {
            check_call(call, context, diags);
        }
    }

    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        if let Expression::FunctionCall(call) = expr {
            check_call(call, context, diags);
        }
    }
}

fn check_call(call: &FunctionCall, context: &LintContext<'_>, diags: &mut Vec<LintDiagnostic>) {
    if !is_global_call(call, "assert", context.semantic) {
        return;
    }
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return;
    };
    let Some(condition) = args.first() else {
        return;
    };
    match condition {
        // 0 and "" are truthy in Lua, so every number and string passes.
        Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::InterpolatedString(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => {
            diags.push(
                LintDiagnostic::new(
                    "unnecessary_assert",
                    "`assert` on a constant value always passes",
                    call.span,
                )
                .with_help("remove the `assert` or check a real condition"),
            );
        }
        Expression::False(_) | Expression::Nil(_) => {
            diags.push(
                LintDiagnostic::new(
                    "unnecessary_assert",
                    "`assert` on a constant falsy value always errors",
                    call.span,
                )
                .with_help("use `error()` to raise unconditionally"),
            );
        }
        Expression::VarArg(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
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
    fn flags_assert_in_expression_position() {
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
