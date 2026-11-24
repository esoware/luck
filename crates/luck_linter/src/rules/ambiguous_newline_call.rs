use luck_ast::{AstKind, AstType, Call, FunctionArgs, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{LuaVersion, Span, has_line_break};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AmbiguousNewlineCall;

declare_lint_rule!(
    AmbiguousNewlineCall,
    Suspicious,
    Warning,
    "call arguments open on a new line, which reads ambiguously"
);

impl Rule for AmbiguousNewlineCall {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (_, call) in chain.calls() {
            if let Some(finding) = check_call(call, context.source, context.semantic.version()) {
                context.diagnostic(finding);
            }
        }
    }
}

fn check_call(call: Call<'_>, source: &[u8], version: LuaVersion) -> Option<Finding> {
    let FunctionArgs::Parenthesized { span: args_span, .. } = call.args else {
        return None;
    };
    let prefix_end = call
        .method
        .as_ref()
        .map_or_else(|| call.callee.span().end, |method_token| method_token.span.end);
    let open_start = args_span.start;
    if open_start <= prefix_end
        || !has_line_break(Span::new(prefix_end, open_start).text(source), version)
    {
        return None;
    }
    Some(
        Finding::new(
            "call arguments start on a new line, but Lua joins this with the previous expression",
            Span::new(open_start, open_start + 1),
        )
        .with_help(
            "keep `(` on the same line as the callee, or add a semicolon if a new statement was \
             intended",
        ),
    )
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "print(1)",
        "\
print(
1,
2
)",
        "\
local f = print
f
\"s\"
f
{}",
    ];

    const FAIL: &[&str] = &[
        // Parses as `local x = f(3)`, though it reads as two statements.
        "\
local f = print
local x = f
(3)",
        "\
local f = print
f
(3)",
        "\
local f = print
local x = f(1)
(2)",
        "\
local t = {}
local x = t:m
(1)",
        "\
local f = print
local x = f -- note
(3)",
        // Lua reads a lone CR as a line break.
        "\
local f = print
f\r(3)",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::AmbiguousNewlineCall, PASS, FAIL).test_and_snapshot();
    }
}
