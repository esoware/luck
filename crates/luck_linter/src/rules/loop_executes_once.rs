use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::cfg::Exit;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags loops whose body `break`s, `return`s, or `error`s on every path, so
/// it runs at most once. Such a loop was usually meant to be an `if` or a
/// `do` block.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LoopExecutesOnce;

declare_lint_rule!(
    LoopExecutesOnce,
    Suspicious,
    Warning,
    "loop body always exits on the first iteration"
);

/// A loop's body is part of its function's graph, which knows each loop's
/// exits.
impl Rule for LoopExecutesOnce {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[
        AstType::WhileLoop,
        AstType::RepeatLoop,
        AstType::NumericFor,
        AstType::GenericFor,
    ]);

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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        (
            "\
local x = 1
local c = 1
while x do if c then break end print() end",
            LuaTarget::Lua54,
        ),
        ("for i = 1, 10 do print(i) end", LuaTarget::Lua54),
        // `continue` starts another iteration.
        ("for _, v in ipairs(t) do if v then continue end return v end", LuaTarget::Luau),
        // The `goto` starts another iteration.
        (
            "for _, v in ipairs(t) do if v then goto continue end do return v end ::continue:: end",
            LuaTarget::Lua54,
        ),
        // The inner `break` lets the outer loop go on.
        ("for i = 1, 3 do while true do if i then break end end end", LuaTarget::Lua54),
        // A local `error` doesn't raise.
        (
            "\
local function error(text) print(text) end
for i = 1, 3 do error(i) end",
            LuaTarget::Lua54,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (
            "\
local x = 1
while x do return 1 end",
            LuaTarget::Lua54,
        ),
        ("for i = 1, 10 do break end", LuaTarget::Lua54),
        (r#"repeat error("x") until true"#, LuaTarget::Lua54),
        ("for k, v in pairs(t) do return v end", LuaTarget::Lua54),
        // Every branch of the `if` returns, so the body never loops.
        (
            "\
local c = 1
while c do if c then return 1 else return 2 end end",
            LuaTarget::Lua54,
        ),
        ("while true do for i = 1, 2 do break end return end", LuaTarget::Lua54),
        ("for i = 1, 3 do if i then break else return end end", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::LoopExecutesOnce, PASS, FAIL).test_and_snapshot();
    }
}
