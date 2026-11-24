use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::MAX_ACTIVE_LOCALS;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::function_node::FunctionNode;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DuplicateParameter;

declare_lint_rule!(
    DuplicateParameter,
    Correctness,
    Error,
    "function declares two parameters with the same name"
);

impl Rule for DuplicateParameter {
    const NODE_TYPES: AstTypesBitset = FunctionNode::NODE_TYPES;

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        if let Some(function) = FunctionNode::of(node, context.nodes) {
            check_params(function, context);
        }
    }
}

/// Flags each parameter named like an earlier one, or `self` in a colon
/// method, which already has an implicit `self`. `_` may repeat, and a Lua
/// 5.5 named vararg counts as the last parameter.
///
/// Parameters are locals, so no Lua compiles a function with more than
/// [`MAX_ACTIVE_LOCALS`] of them, and one that has more isn't checked.
fn check_params(function: FunctionNode<'_>, context: &LintContext<'_>) {
    let params = &function.body.params;
    let vararg_name = function.body.vararg.as_deref().and_then(|vararg| vararg.name.as_ref());
    if params.len() + usize::from(vararg_name.is_some()) > MAX_ACTIVE_LOCALS {
        return;
    }
    for (i, param_name) in params.iter().map(|param| &param.name).chain(vararg_name).enumerate() {
        let name = param_name.text;
        if name == "_" {
            continue;
        }
        let first = params.iter().take(i).find(|earlier| earlier.name.text == name);
        if first.is_some() || (function.is_method && name == "self") {
            let finding =
                Finding::new(format!("parameter `{name}` is already defined"), param_name.span);
            context.diagnostic(match first {
                Some(earlier) => finding.with_label(earlier.name.span, "first defined here"),
                None => finding.with_help("a method declared with `:` already has `self`"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    /// A function of `count` parameters, one a line, then the first again.
    fn params_then_the_first_again(count: u32) -> String {
        let params: Vec<String> = (0..count).map(|i| format!("p{i},")).collect();
        format!("local function f(\n{}\np0) end", params.join("\n"))
    }

    const PASS: &[(&str, LuaTarget)] = &[
        ("local function f(a, b, c) end", LuaTarget::Lua54),
        ("local function f(_, _) end", LuaTarget::Lua54),
        (
            "\
local t = {}
function t.m(self) end",
            LuaTarget::Lua54,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("local function f(a, b, a) end", LuaTarget::Lua54),
        ("local f = function(x, x) end", LuaTarget::Lua54),
        ("function f(a, a) end", LuaTarget::Lua54),
        ("local function f(a, a, a) end", LuaTarget::Lua54),
        (
            "\
local t = {}
function t:m(self) end",
            LuaTarget::Lua54,
        ),
        ("global function f(a, a) end", LuaTarget::Lua55),
        // The named vararg repeats parameter `a`.
        ("local function f(a, ...a) return a end", LuaTarget::Lua55),
    ];

    #[test]
    fn test() {
        let at_limit = params_then_the_first_again(199);
        // Only Luau's compiler, not luck, rejects this many parameters.
        let past_limit = params_then_the_first_again(200);
        let pass: Vec<_> =
            PASS.iter().copied().chain([(past_limit.as_str(), LuaTarget::Luau)]).collect();
        let fail: Vec<_> =
            FAIL.iter().copied().chain([(at_limit.as_str(), LuaTarget::Lua54)]).collect();
        Tester::new(RuleId::DuplicateParameter, &pass, &fail).test_and_snapshot();
    }
}
