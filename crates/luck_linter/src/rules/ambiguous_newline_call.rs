use luck_ast::{AstKind, AstType, Call, FunctionArgs, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{LuaVersion, Span, has_line_break};

use crate::call::calls_in;
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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (_, call) in calls_in(chain) {
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
            Span::new(open_start, open_start.saturating_add(1)),
        )
        .with_help(
            "keep `(` on the same line as the callee, or add a semicolon if a new statement was \
             intended",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::AmbiguousNewlineCall, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_paren_on_next_line() {
        // Parses as `local x = f(3)`, though it reads as two statements.
        let diags = run("\
local f = print
local x = f
(3)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_stmt_level_split_call() {
        let diags = run("\
local f = print
f
(3)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_split_chained_call() {
        let diags = run("\
local f = print
local x = f(1)
(2)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_split_method_call() {
        let diags = run("\
local t = {}
local x = t:m
(1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_with_comment_in_gap() {
        let diags = run("\
local f = print
local x = f -- note
(3)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_same_line_call() {
        let diags = run("print(1)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_multiline_args() {
        let diags = run("\
print(
1,
2
)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_string_and_table_calls() {
        let diags = run("\
local f = print
f
\"s\"
f
{}");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_paren_after_cr_only_line_break() {
        let diags = run("local f = print\nf\r(3)");
        assert_eq!(diags.len(), 1, "Lua reads a lone CR as a line break: {diags:?}");
    }
}
