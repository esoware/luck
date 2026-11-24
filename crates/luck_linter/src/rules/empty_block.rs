use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EmptyBlock;

declare_lint_rule!(EmptyBlock, Suspicious, Warning, "block body is empty");

impl Rule for EmptyBlock {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[
        AstType::IfStatement,
        AstType::ElseIfClause,
        AstType::ElseClause,
        AstType::WhileLoop,
        AstType::DoBlock,
        AstType::NumericFor,
        AstType::GenericFor,
        AstType::RepeatLoop,
    ]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let kind = node.kind();
        let (block, message, span) = if let AstKind::IfStatement(if_stmt) = kind {
            (&if_stmt.block, "empty `if` block", if_stmt.span)
        } else if let AstKind::ElseIfClause(clause) = kind {
            (&clause.block, "empty `elseif` block", clause.span)
        } else if let AstKind::ElseClause(clause) = kind {
            (&clause.block, "empty `else` block", clause.span)
        } else if let AstKind::WhileLoop(while_loop) = kind {
            (&while_loop.block, "empty `while` loop body", while_loop.span)
        } else if let AstKind::DoBlock(do_block) = kind {
            (&do_block.block, "empty `do` block", do_block.span)
        } else if let AstKind::NumericFor(numeric_for) = kind {
            (&numeric_for.block, "empty numeric `for` loop body", numeric_for.span)
        } else if let AstKind::GenericFor(generic_for) = kind {
            (&generic_for.block, "empty generic `for` loop body", generic_for.span)
        } else if let AstKind::RepeatLoop(repeat_loop) = kind {
            (&repeat_loop.block, "empty `repeat` loop body", repeat_loop.span)
        } else {
            return;
        };
        if block.stmts.is_empty() {
            context.diagnostic(Finding::new(message, span));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "do return end",
        "\
local c = 1
if c then print(1) else print(2) end
while c do break end",
    ];

    const FAIL: &[&str] = &[
        "\
local c = 1
if c then elseif c then else end",
        "\
local c = 1
while c do end
do end
for i = 1, 2 do end
for k in pairs({}) do end
repeat until c",
        "\
local c = 1
if c then ; else ;; end
do ; end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::EmptyBlock, PASS, FAIL).test_and_snapshot();
    }
}
