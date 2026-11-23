use luck_ast::{Call, Expression, GetSpan as _};
use luck_diagnostics::Tag;
use luck_semantic::SemanticAnalysis;

use crate::call::{CallArg, CallArgs, is_global_call};
use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallSite, LintContext, NodeRule, impl_rule};

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
    fn on_call(&self, site: &CallSite<'_>, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let call = site.call;
        findings.extend(check_call(call, context.semantic));
    }
}

fn check_call(call: Call<'_>, semantic: &SemanticAnalysis<'_>) -> Option<Finding> {
    if !is_global_call(call, "assert", semantic) {
        return None;
    }
    let is_truthy = match CallArgs::new(call.args).next()? {
        CallArg::String(_) | CallArg::Table(_) => true,
        CallArg::Expr(expr) => match expr {
            // 0 and "" are truthy in Lua, so every number and string passes.
            Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::InterpolatedString(_)
            | Expression::TableConstructor(_)
            | Expression::FunctionDef(_) => true,
            Expression::False(_) | Expression::Nil(_) => false,
            Expression::VarArg(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => return None,
        },
    };
    let finding = if is_truthy {
        Finding::new("`assert` on a constant value always passes", call.span())
            .with_tag(Tag::Unnecessary)
            .with_help("remove the `assert` or check a real condition")
    } else {
        Finding::new("`assert` on a constant falsy value always errors", call.span())
            .with_help("use `error()` to raise unconditionally")
    };
    Some(finding)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
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
    fn flags_string_and_table_arguments_without_parentheses() {
        for source in ["assert\"\"", "assert'x'", "assert[[x]]", "assert{}", "assert{ false }"] {
            let diags = run(source);
            assert_eq!(diags.len(), 1, "{source}: {diags:?}");
            assert!(diags[0].message.contains("always passes"), "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_assert_in_expr_position() {
        let diags = run("local ok = assert(true)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_variable_condition() {
        let diags = run("\
local x = 1
assert(x)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_call_condition() {
        let diags = run("assert(next({}))");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_assert() {
        let diags = run("\
local assert = function() end
assert(true)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_empty_assert() {
        let diags = run("assert()");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
