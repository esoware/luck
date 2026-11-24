use luck_ast::{AstKind, AstType, GetSpan as _, Statement};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;

use crate::cfg::FlowGraph;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags code no path through its function's control-flow graph reaches.
///
/// That's code after a `return`, `break`, `goto`, call of the host's `error`,
/// or loop that never ends, after a branch every path of which leaves, and
/// in a `while` whose condition is always false.
///
/// Each run of unreachable statements in a block is one finding. A block
/// whose statement is unreachable is part of that statement's finding, and a
/// function's body has its own graph, so a function defined in unreachable
/// code is reported once, as the code that defines it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnreachableCode;

declare_lint_rule!(
    UnreachableCode,
    Suspicious,
    Warning,
    "code that no path through its function reaches"
);

impl Rule for UnreachableCode {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        let Some(graph) = context.cfg(node.scope()) else {
            return;
        };
        let mut can_reach = is_entered(node, graph, context);
        let mut run: Option<Span> = None;
        for stmt in &block.stmts {
            if graph.can_run(stmt.span().start) {
                can_reach = true;
                report(run.take(), context);
                continue;
            }
            if !can_reach || is_declaration(stmt) {
                continue;
            }
            let span = stmt.span();
            run = Some(run.map_or(span, |reported| reported.merge(span)));
        }
        report(run, context);
    }
}

/// Whether the code before `block`'s first statement can run: a function's
/// or the chunk's always does, and an `if` clause runs where its `if` does.
fn is_entered(block: &AstNode<'_>, graph: &FlowGraph, context: &LintContext<'_>) -> bool {
    let nodes = context.nodes;
    let Some(parent) = nodes.parent_node(block.id()) else {
        return true;
    };
    let kind = parent.kind();
    if matches!(kind, AstKind::FunctionBody(_) | AstKind::Chunk(_)) {
        return true;
    }
    let owner = if matches!(kind, AstKind::ElseIfClause(_) | AstKind::ElseClause(_)) {
        nodes.parent_node(parent.id()).expect("a clause is inside its `if`")
    } else {
        parent
    };
    graph.can_run(owner.span().start)
}

/// A statement the compiler reads without emitting code for it, which no
/// path running past it changes: a Luau type, which its whole block sees,
/// or a Lua 5.5 `global` declaration that assigns nothing.
fn is_declaration(stmt: &Statement<'_>) -> bool {
    match stmt {
        // Luau, Lua 5.5
        Statement::TypeDeclaration(_) | Statement::GlobalStar(_) => true,
        // Lua 5.5
        Statement::GlobalDeclaration(declaration) => declaration.exprs.is_empty(),
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::DoBlock(_)
        | Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::IfStatement(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::LocalAssignment(_)
        | Statement::Return(_)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalFunction(_)
        | Statement::CompoundAssignment(_)
        | Statement::Error(_) => false,
    }
}

fn report(run: Option<Span>, context: &LintContext<'_>) {
    if let Some(span) = run {
        context.diagnostic(Finding::new("unreachable code", span).with_tag(Tag::Unnecessary));
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
local function error() end
error('go on')
print(1)",
            LuaTarget::Lua54,
        ),
        ("while true do print(1) break end", LuaTarget::Lua54),
        (
            "\
local x = 1
print(x)",
            LuaTarget::Lua54,
        ),
        // A `;` separator isn't code.
        ("while true do break; end", LuaTarget::Lua54),
        // The `goto` reaches the label after `break`.
        ("while c do if x then goto a end break ::a:: print(1) end", LuaTarget::Lua54),
        (
            "\
local function f(x: T)
  do return end
  type T = number
end",
            LuaTarget::Luau,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        // From Lua 5.2 a `break` can sit mid-block, so a statement can follow it.
        ("while true do break print(1) end", LuaTarget::Lua54),
        ("for i = 1, 3 do break print(1) print(2) end", LuaTarget::Lua54),
        (
            "\
local function f()
  do return end
  print(1)
end",
            LuaTarget::Lua51,
        ),
        (
            "\
error('stop')
print(1)",
            LuaTarget::Luau,
        ),
        (
            "\
while true do print(1) end
print(2)",
            LuaTarget::Lua54,
        ),
        (
            "\
local function f(x)
  if x then return 1 else return 2 end
  print(x)
end",
            LuaTarget::Lua54,
        ),
        (
            "\
goto skip
print(1)
::skip::
print(2)",
            LuaTarget::Lua54,
        ),
        ("while false do print(1) end", LuaTarget::Lua54),
        (
            "\
local function f(x)
  do return end
  if x then print(x) end
end",
            LuaTarget::Lua54,
        ),
        (
            "\
do return end
local function g() print(1) end",
            LuaTarget::Lua54,
        ),
        ("while c do break ::a:: print(1) end", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnreachableCode, PASS, FAIL).test_and_snapshot();
    }
}
