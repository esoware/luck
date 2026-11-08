use luck_ast::{Expression, FunctionArgs, FunctionCall, Statement, Var};
use luck_semantic::SemanticAnalysis;
use luck_token::BinOp;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

/// Flags `type(x == "string")` written for `type(x) == "string"`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypeCheckInsideCall;

impl Rule for TypeCheckInsideCall {
    fn name(&self) -> &'static str {
        "type_check_inside_call"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "type comparison is inside the call instead of outside"
    }
}

impl NodeRule for TypeCheckInsideCall {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Statement::FunctionCall(call) = stmt {
            findings.extend(check_call(call, context.semantic));
        }
    }
    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Expression::FunctionCall(call) = expr {
            findings.extend(check_call(call, context.semantic));
        }
    }
}

fn check_call(call: &FunctionCall, semantic: &SemanticAnalysis) -> Option<Finding> {
    let Expression::Var(Var::Name(token)) = &call.callee else {
        return None;
    };
    let name = token.identifier()?;
    // A local `type` or `typeof` is a user function.
    if !matches!(name, "type" | "typeof") || semantic.resolves_to_local(name, token.span) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return None;
    };
    let [Expression::BinaryOp(binary)] = args.items.as_slice() else {
        return None;
    };
    matches!(binary.op, BinOp::Eq | BinOp::Ne).then(|| {
        Finding::new(
            "comparison is inside the `type()` call, did you mean `type(x) == \"string\"`?",
            call.span,
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
