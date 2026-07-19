use luck_ast::shared::Block;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;
use luck_token::Span;

use crate::cfg::{Exit, analyze_full_block};
use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Detects `while`/`repeat`/`for` loops whose body unconditionally
/// terminates on every path - making the loop body execute at most
/// once. A loop that always `break`s, `return`s, or `error`s is almost
/// certainly a typo (likely an `if`/`do` block was intended).
pub struct LoopExecutesOnce;

impl Rule for LoopExecutesOnce {
    fn name(&self) -> &'static str {
        "loop_executes_once"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "loop body always exits on the first iteration"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = LoopChecker {
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct LoopChecker {
    diagnostics: Vec<LintDiagnostic>,
}

impl LoopChecker {
    fn check_loop_body(&mut self, body: &Block, loop_span: Span, kind: &'static str) {
        let summary = analyze_full_block(body);
        // The body must terminate on every path - that means the
        // analyzer pinned a non-Normal exit on the sequence.
        let label = match summary.exit {
            Exit::Return => "always returns",
            Exit::Break => "always breaks",
            Exit::Error => "always errors",
            Exit::Continue | Exit::Normal => return,
        };
        self.diagnostics.push(
            LintDiagnostic::new(
                "loop_executes_once",
                format!("{kind} loop body {label}; loop executes at most once"),
                loop_span,
            )
            .with_help("if a single execution is intended, use a `do ... end` block".to_string()),
        );
    }
}

impl Visitor for LoopChecker {
    fn visit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::WhileLoop(node) => {
                self.check_loop_body(&node.block, node.span, "while");
            }
            Statement::RepeatLoop(node) => {
                self.check_loop_body(&node.block, node.span, "repeat");
            }
            Statement::NumericFor(node) => {
                self.check_loop_body(&node.block, node.span, "numeric for");
            }
            Statement::GenericFor(node) => {
                self.check_loop_body(&node.block, node.span, "generic for");
            }
            _ => {}
        }
        self.walk_statement(stmt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&LoopExecutesOnce, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_while_with_unconditional_return() {
        let diags = run("local x = 1\nwhile x do return 1 end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("returns"));
    }

    #[test]
    fn flags_numeric_for_with_break() {
        let diags = run("for i = 1, 10 do break end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("breaks"));
    }

    #[test]
    fn flags_repeat_with_error() {
        let diags = run("repeat error(\"x\") until true");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("errors"));
    }

    #[test]
    fn ignores_when_some_path_continues() {
        let diags = run("local x = 1\nlocal c = 1\nwhile x do if c then break end print() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_normal_loop() {
        let diags = run("for i = 1, 10 do print(i) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_generic_for_with_return() {
        let diags = run("for k, v in pairs(t) do return v end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_when_if_else_both_return() {
        // The body's exit becomes Return when every if/else branch
        // returns.
        let diags = run("local c = 1\nwhile c do if c then return 1 else return 2 end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }
}
