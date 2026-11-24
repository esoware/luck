use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::cfg::Exit;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{RuleRunner, impl_rule};

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

/// A loop's body is part of its function's graph, which knows each loop's
/// exits.
impl RuleRunner for LoopExecutesOnce {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[
            AstType::WhileLoop,
            AstType::RepeatLoop,
            AstType::NumericFor,
            AstType::GenericFor,
        ])
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let kind = node.kind();
        let (span, loop_name) = if let AstKind::WhileLoop(while_loop) = kind {
            (while_loop.span, "`while`")
        } else if let AstKind::RepeatLoop(repeat_loop) = kind {
            (repeat_loop.span, "`repeat`")
        } else if let AstKind::NumericFor(numeric_for) = kind {
            (numeric_for.span, "numeric `for`")
        } else if let AstKind::GenericFor(generic_for) = kind {
            (generic_for.span, "generic `for`")
        } else {
            return;
        };
        let Some(exits) = context.cfg(node.scope()).and_then(|graph| graph.loop_exits(span.start))
        else {
            return;
        };
        if exits.reaches(Exit::End) {
            return;
        }
        let can_return = exits.reaches(Exit::ReturnValue) || exits.reaches(Exit::ReturnEmpty);
        let label = match (can_return, exits.reaches(Exit::Break), exits.reaches(Exit::Error)) {
            (true, false, _) => "always returns",
            (false, true, false) => "always breaks",
            // A body stuck in an inner infinite loop reaches no exit, and
            // never starts another iteration either.
            (false, false, _) => "always errors",
            (true, true, _) | (false, true, true) => "always exits",
        };
        context.diagnostic(
            Finding::new(
                format!("{loop_name} loop body {label}, so the loop executes at most once"),
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
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
        run_rule(&LoopExecutesOnce, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_while_with_unconditional_return() {
        let diags = run("\
local x = 1
while x do return 1 end");
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
        let diags = run("\
local x = 1
local c = 1
while x do if c then break end print() end");
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
        let diags = run("\
local c = 1
while c do if c then return 1 else return 2 end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_nested_loops_outer_first() {
        let diags = run("while true do for i = 1, 2 do break end return end");
        let messages: Vec<&str> = diags.iter().map(|diag| &*diag.message).collect();
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
    fn ignores_loop_with_conditional_continue() {
        let source = "for _, v in ipairs(t) do if v then continue end return v end";
        let diags = run_rule(&LoopExecutesOnce, source, LuaVersion::Luau);
        assert!(diags.is_empty(), "`continue` starts another iteration: {diags:?}");
    }

    #[test]
    fn ignores_loop_with_goto_continue() {
        let diags = run(
            "for _, v in ipairs(t) do if v then goto continue end do return v end ::continue:: end",
        );
        assert!(diags.is_empty(), "the `goto` starts another iteration: {diags:?}");
    }

    #[test]
    fn ignores_outer_loop_around_while_true_with_conditional_break() {
        let diags = run("for i = 1, 3 do while true do if i then break end end end");
        assert!(diags.is_empty(), "the inner `break` lets the outer loop go on: {diags:?}");
    }

    #[test]
    fn ignores_shadowed_error_call() {
        let diags = run("\
local function error(text) print(text) end
for i = 1, 3 do error(i) end");
        assert!(diags.is_empty(), "a local `error` doesn't raise: {diags:?}");
    }

    #[test]
    fn flags_body_that_always_exits_one_of_two_ways() {
        let diags = run("for i = 1, 3 do if i then break else return end end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("always exits"), "got: {diags:?}");
    }
}
