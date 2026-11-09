use luck_ast::Statement;

use crate::cfg::{Exit, analyze_full_block};
use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags loops whose body `break`s, `return`s, or `error`s on every path, so
/// it runs at most once. Such a loop was usually meant to be an `if` or a
/// `do` block.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoopExecutesOnce;

impl_rule!(
    LoopExecutesOnce,
    "loop_executes_once",
    Suspicious,
    Warning,
    "loop body always exits on the first iteration"
);

impl NodeRule for LoopExecutesOnce {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[
            NodeType::WhileLoop,
            NodeType::RepeatLoop,
            NodeType::NumericFor,
            NodeType::GenericFor,
        ])
    }
    fn on_stmt(&self, stmt: &Statement, _context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let (body, span, kind) = match stmt {
            Statement::WhileLoop(node) => (&node.block, node.span, "`while`"),
            Statement::RepeatLoop(node) => (&node.block, node.span, "`repeat`"),
            Statement::NumericFor(node) => (&node.block, node.span, "numeric `for`"),
            Statement::GenericFor(node) => (&node.block, node.span, "generic `for`"),
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::IfStatement(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => return,
        };
        let label = match analyze_full_block(body).exit {
            Exit::Return => "always returns",
            Exit::Break => "always breaks",
            Exit::Error => "always errors",
            Exit::Continue | Exit::Normal => return,
        };
        findings.push(
            Finding::new(
                format!("{kind} loop body {label}, so the loop executes at most once"),
                span,
            )
            .with_help("if a single execution is intended, use a `do ... end` block"),
        );
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&LoopExecutesOnce, source, LuaVersion::Lua54)
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
    fn ignores_when_some_path_falls_through() {
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

    #[test]
    fn flags_nested_loops_outer_first() {
        let diags = run("while true do for i = 1, 2 do break end return end");
        let messages: Vec<&str> = diags.iter().map(|diag| diag.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "`while` loop body always returns, so the loop executes at most once",
                "numeric `for` loop body always breaks, so the loop executes at most once",
            ],
            "{diags:?}"
        );
    }

    #[test]
    fn bug_ignores_loop_with_conditional_continue() {
        let source = "for _, v in ipairs(t) do if v then continue end return v end";
        let diags = run_rule(&LoopExecutesOnce, source, LuaVersion::Luau);
        assert!(diags.is_empty(), "`continue` starts another iteration: {diags:?}");
    }

    #[test]
    fn bug_ignores_loop_with_goto_continue() {
        let diags = run(
            "for _, v in ipairs(t) do if v then goto continue end do return v end ::continue:: end",
        );
        assert!(diags.is_empty(), "the `goto` starts another iteration: {diags:?}");
    }

    #[test]
    fn bug_ignores_outer_loop_around_while_true_with_conditional_break() {
        let diags = run("for i = 1, 3 do while true do if i then break end end end");
        assert!(diags.is_empty(), "the inner `break` lets the outer loop go on: {diags:?}");
    }

    #[test]
    fn bug_ignores_shadowed_error_call() {
        let diags = run("local function error(text) print(text) end\nfor i = 1, 3 do error(i) end");
        assert!(diags.is_empty(), "a local `error` doesn't raise: {diags:?}");
    }
}
