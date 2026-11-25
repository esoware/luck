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
/// That's code after a call of the host's `error`, a loop that never ends,
/// or a statement every path through which leaves by `return`, `break`, or
/// `continue`, and code in a `while` whose condition is always false.
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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
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
            // A type declaration emits no code, and its whole block sees it.
            if !can_reach || matches!(stmt, Statement::TypeDeclaration(_)) {
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
fn is_entered(block: &AstNode<'_>, graph: &FlowGraph, context: &LintContext<'_, '_>) -> bool {
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

fn report(run: Option<Span>, context: &LintContext<'_, '_>) {
    if let Some(span) = run {
        context.diagnostic(Finding::new("unreachable code", span).with_tag(Tag::Unnecessary));
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local function error() end
error('go on')
print(1)",
        "while true do print(1) break end",
        "\
local x = 1
print(x)",
        // A `;` separator isn't code.
        "while true do break; end",
        "for i = 1, 3 do if i then continue end print(i) end",
        "\
local function f(x: T)
  do return end
  type T = number
end",
    ];

    const FAIL: &[&str] = &[
        "for i = 1, 3 do if i then break else continue end print(i) end",
        "\
local function f()
  do return end
  print(1)
end",
        "\
error('stop')
print(1)",
        "\
while true do print(1) end
print(2)",
        "\
local function f(x)
  if x then return 1 else return 2 end
  print(x)
end",
        "while false do print(1) end",
        "\
local function f(x)
  do return end
  if x then print(x) end
end",
        "\
do return end
local function g() print(1) end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnreachableCode, PASS, FAIL).test_and_snapshot();
    }
}
