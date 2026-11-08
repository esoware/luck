use luck_ast::{Expression, FunctionArgs, FunctionCall, NodeType, NodeTypeSet, Statement};
use luck_token::Span;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct AmbiguousNewlineCall;

impl Rule for AmbiguousNewlineCall {
    fn name(&self) -> &'static str {
        "ambiguous_newline_call"
    }

    fn category(&self) -> Category {
        Category::Suspicious
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "call arguments open on a new line, which reads ambiguously"
    }
}

impl NodeRule for AmbiguousNewlineCall {
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
    let FunctionArgs::Parenthesized { span: args_span, .. } = &call.args else {
        return;
    };
    let prefix_end = call
        .method
        .as_ref()
        .map_or_else(|| call.callee.span().end, |method_token| method_token.span.end);
    let open_start = args_span.start;
    if open_start <= prefix_end {
        return;
    }
    let gap = Span::new(prefix_end, open_start).text(context.source);
    if !gap.contains('\n') {
        return;
    }
    diags.push(
        LintDiagnostic::new(
            "ambiguous_newline_call",
            "call arguments start on a new line, but Lua joins this with the previous expression",
            Span::new(open_start, open_start.saturating_add(1)),
        )
        .with_help(
            "keep `(` on the same line as the callee, or add a semicolon if a new statement was \
             intended",
        ),
    );
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
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
    fn flags_statement_level_split_call() {
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
}
