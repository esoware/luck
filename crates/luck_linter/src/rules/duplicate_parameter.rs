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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        if let Some(function) = FunctionNode::of(node, context.nodes) {
            check_params(function, context);
        }
    }
}

/// Flags each parameter named like an earlier one, or `self` in a colon
/// method, which already has an implicit `self`. `_` may repeat.
///
/// Parameters are locals, so Luau compiles no function with more than
/// [`MAX_ACTIVE_LOCALS`] of them, and one that has more isn't checked.
fn check_params(function: FunctionNode<'_>, context: &LintContext<'_, '_>) {
    let params = &function.body.params;
    if params.len() > MAX_ACTIVE_LOCALS {
        return;
    }
    for (i, param) in params.iter().enumerate() {
        let name = param.name.text;
        if name == "_" {
            continue;
        }
        let first = params.iter().take(i).find(|earlier| earlier.name.text == name);
        if first.is_some() || (function.is_method && name == "self") {
            let finding =
                Finding::new(format!("parameter `{name}` is already defined"), param.name.span);
            context.diagnostic(match first {
                Some(earlier) => finding.with_label(earlier.name.span, "first defined here"),
                None => finding.with_help("a method declared with `:` already has `self`"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    /// A function of `count` parameters, one a line, then the first again.
    fn params_then_the_first_again(count: usize) -> String {
        let mut source = String::new();
        writeln!(source, "local function f(").unwrap();
        for i in 0..count {
            writeln!(source, "p{i},").unwrap();
        }
        source.push_str("p0) end");
        source
    }

    const PASS: &[&str] = &[
        "local function f(a, b, c) end",
        "local function f(_, _) end",
        "\
local t = {}
function t.m(self) end",
    ];

    const FAIL: &[&str] = &[
        "local function f(a, b, a) end",
        "local f = function(x, x) end",
        "function f(a, a) end",
        "local function f(a, a, a) end",
        "\
local t = {}
function t:m(self) end",
    ];

    #[test]
    fn test() {
        let at_limit = params_then_the_first_again(199);
        // Only Luau's compiler, not luck, rejects this many parameters.
        let past_limit = params_then_the_first_again(200);
        let pass: Vec<_> = PASS.iter().copied().chain([past_limit.as_str()]).collect();
        let fail: Vec<_> = FAIL.iter().copied().chain([at_limit.as_str()]).collect();
        Tester::new(RuleId::DuplicateParameter, &pass, &fail).test_and_snapshot();
    }
}
