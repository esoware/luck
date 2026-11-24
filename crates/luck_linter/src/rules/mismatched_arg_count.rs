use luck_ast::{AstKind, AstType, Call, Expression, FunctionBody, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset, Symbol};

use crate::call::{CallArg, CallArgs};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MismatchedArgCount;

declare_lint_rule!(
    MismatchedArgCount,
    Correctness,
    Warning,
    "call passes more arguments than the called function accepts"
);

impl Rule for MismatchedArgCount {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (_, call) in chain.calls() {
            if let Some(finding) = check_call(call, context) {
                context.diagnostic(finding);
            }
        }
    }
}

/// The function a local is bound to where it's declared: a `local function`,
/// or a function literal its `local` gives it.
fn declared_function<'ast>(
    symbol: &Symbol,
    context: &LintContext<'_, 'ast>,
) -> Option<&'ast FunctionBody<'ast>> {
    if let AstKind::LocalFunction(local_function) = context.nodes.kind(symbol.declaration) {
        return Some(&local_function.body);
    }
    let (local, position) = context.local_declaration(symbol)?;
    let Some(Expression::FunctionDef(function)) = local.exprs.get(position) else {
        return None;
    };
    Some(&function.body)
}

/// A local that's ever reassigned is left out, since it could hold any
/// function by the time a call runs.
fn check_call(call: Call<'_>, context: &LintContext<'_, '_>) -> Option<Finding> {
    if call.method.is_some() {
        return None;
    }
    let tree = context.scoping();
    let callee = call.callee.as_name()?;
    let symbol = tree.symbol(tree.resolved_symbol_of(callee)?);
    if symbol.is_modified {
        return None;
    }
    let body = declared_function(symbol, context)?;
    if body.vararg.is_some() {
        return None;
    }
    let param_count = body.params.len();
    let args = CallArgs::new(call.args);
    let arg_count = args.len();
    // A trailing call or `...` can expand to nothing, so only the args
    // before it are sure to be passed.
    let has_multi_value_tail = args.last().is_some_and(CallArg::is_multi_value);
    let guaranteed_arg_count = arg_count - usize::from(has_multi_value_tail);
    if guaranteed_arg_count <= param_count {
        return None;
    }
    Some(
        Finding::new(
            format!(
                "call passes {guaranteed_arg_count} arguments to function `{}`, \
                 which accepts only {param_count}",
                callee.text
            ),
            call.span(),
        )
        .with_label(symbol.definition_span, "defined here")
        .with_help("extra arguments are silently discarded"),
    )
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local function f(a, b) return a + b end
f(1, 2)",
        // Missing args are nil, which is how Lua does optional arguments.
        "\
local function f(a, b) return a or b end
f(1)",
        "\
local function f(...) return ... end
f(1, 2, 3, 4)",
        // `g()` may return nothing, so `f(1, g())` passes one argument for sure.
        "\
local function g() end
local function f(a) return a end
f(1, g())",
        "\
local function f(a) return a end
f = function(a, b, c) return a end
f(1, 2, 3)",
        "print(1, 2, 3)",
        "\
local t = { f = function(a) return a end }
t.f(1, 2)",
        "\
local function f(a) return a end
do local f = print
f(1, 2) end",
        "\
local function f(a)
    if a > 0 then return f(a - 1) end
end
f(3)",
    ];

    const FAIL: &[&str] = &[
        "\
local function f(a, b) return a + b end
f(1, 2, 3)",
        "\
local f = function(a) return a end
f(1, 2)",
        "\
local function f() end
f(1)",
        // `g()` may return nothing, but `1` and `2` already exceed one parameter.
        "\
local function g() end
local function f(a) return a end
f(1, 2, g())",
        "\
local function f() end
f\"x\"
f{}",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::MismatchedArgCount, PASS, FAIL).test_and_snapshot();
    }
}
