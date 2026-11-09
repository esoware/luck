use luck_ast::{FunctionArgs, FunctionCall};
use luck_token::{LuaVersion, Span, has_line_break};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct AmbiguousNewlineCall;

impl_rule!(
    AmbiguousNewlineCall,
    "ambiguous_newline_call",
    Suspicious,
    Warning,
    "call arguments open on a new line, which reads ambiguously"
);

impl NodeRule for AmbiguousNewlineCall {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr])
    }
    fn on_call(
        &self,
        call: &FunctionCall,
        _position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        findings.extend(check_call(call, context.source, context.semantic.version));
    }
}

fn check_call(call: &FunctionCall, source: &str, version: LuaVersion) -> Option<Finding> {
    let FunctionArgs::Parenthesized { span: args_span, .. } = &call.args else {
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
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&AmbiguousNewlineCall, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_paren_on_next_line() {
        // Parses as `local x = f(3)`, though it reads as two statements.
        let diags = run("local f = print\nlocal x = f\n(3)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_stmt_level_split_call() {
        let diags = run("local f = print\nf\n(3)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_split_chained_call() {
        let diags = run("local f = print\nlocal x = f(1)\n(2)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_split_method_call() {
        let diags = run("local t = {}\nlocal x = t:m\n(1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_with_comment_in_gap() {
        let diags = run("local f = print\nlocal x = f -- note\n(3)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_same_line_call() {
        let diags = run("print(1)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_multiline_args() {
        let diags = run("print(\n1,\n2\n)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_string_and_table_calls() {
        let diags = run("local f = print\nf\n\"s\"\nf\n{}");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_paren_after_cr_only_line_break() {
        let diags = run("local f = print\nf\r(3)");
        assert_eq!(diags.len(), 1, "Lua reads a lone CR as a line break: {diags:?}");
    }
}
