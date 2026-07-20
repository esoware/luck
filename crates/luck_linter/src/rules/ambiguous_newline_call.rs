use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall};
use luck_token::Span;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct AmbiguousNewlineCall;

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

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for AmbiguousNewlineCall {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::stmt::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::stmt::Statement::FunctionCall(call_stmt) = stmt {
            check_call(&call_stmt.call, ctx, out);
        }
    }

    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::FunctionCall(call) = expr {
            check_call(call, ctx, out);
        }
    }
}

fn check_call(call: &FunctionCall, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
    let FunctionArgs::Parenthesized {
        span: args_span, ..
    } = &call.args
    else {
        return;
    };
    let prefix_end = match &call.method {
        Some(method_token) => method_token.span.end,
        None => call.callee.span().end,
    };
    let open_start = args_span.start;
    if open_start <= prefix_end {
        return;
    }
    let gap = &ctx.source[prefix_end as usize..open_start as usize];
    if !gap.contains('\n') {
        return;
    }
    out.push(
        LintDiagnostic::new(
            "ambiguous_newline_call",
            "call arguments start on a new line; Lua joins this with the previous expression",
            Span::new(args_span.start, args_span.start + 1),
        )
        .with_help(
            "keep `(` on the same line as the callee, or add a semicolon if a new statement was \
             intended"
                .to_string(),
        ),
    );
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::AmbiguousNewlineCall;
    use crate::diagnostic::LintDiagnostic;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&AmbiguousNewlineCall, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_paren_on_next_line() {
        // Parses as `local x = f(3)` even though it reads as two
        // statements - exactly the ambiguity being flagged.
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
