use luck_ast::{AstKind, AstType, Call, GetSpan as _};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};

use crate::call::{CallArgs, calls_in, is_global_call};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnnecessaryAssert;

impl_rule!(
    UnnecessaryAssert,
    "unnecessary_assert",
    Suspicious,
    Warning,
    "`assert` on a constant value always passes or always errors"
);

impl RuleRunner for UnnecessaryAssert {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::SuffixedExpression])
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (_, call) in calls_in(chain) {
            if let Some(finding) = check_call(call, context) {
                context.diagnostic(finding);
            }
        }
    }
}

fn check_call(call: Call<'_>, context: &LintContext<'_>) -> Option<Finding> {
    if !is_global_call(call, "assert", context.semantic) {
        return None;
    }
    let is_truthy = CallArgs::new(call.args).next()?.to_boolean(context)?;
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
    fn flags_an_argument_whose_truthiness_is_known() {
        for (source, message) in [
            ("assert(1 + 1 == 2)", "always passes"),
            ("assert(x or true)", "always passes"),
            ("assert(not 1)", "always errors"),
        ] {
            let diags = run(source);
            assert_eq!(diags.len(), 1, "{source}: {diags:?}");
            assert!(diags[0].message.contains(message), "{source}: {diags:?}");
        }
    }

    #[test]
    fn ignores_an_interpolated_string_with_an_expression_on_luau() {
        // Its `format` comes through the string metatable, which a program
        // can change.
        let diags = run_rule(&UnnecessaryAssert, "assert(`a{x}`)", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
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
