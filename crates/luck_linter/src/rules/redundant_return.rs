use luck_ast::{AstKind, AstType, Statement};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RedundantReturn;

declare_lint_rule!(
    RedundantReturn,
    Style,
    Warning,
    "trailing bare `return` has no effect",
    fix = Fix
);

/// Only the chunk and function bodies are checked, since in a nested block a
/// bare `return` is an early exit.
impl Rule for RedundantReturn {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        if !matches!(
            context.nodes.parent_kind(node.id()),
            Some(AstKind::FunctionBody(_) | AstKind::Chunk(_))
        ) {
            return;
        }
        let Some(Statement::Return(return_stmt)) = block.stmts.last() else {
            return;
        };
        if !return_stmt.exprs.is_empty() {
            return;
        }
        let span = return_stmt.span;
        context.diagnostic(
            Finding::new("bare `return` at the end of the function has no effect", span)
                .with_tag(Tag::Unnecessary)
                .with_fix(context, FixKind::Fix, |fixer| {
                    fixer.delete("remove the redundant `return`", span)
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local function f()
    return 1
end
f()",
        "\
local function f(x)
    if x then
        return
    end
    print(x)
end
f(1)",
    ];

    const FAIL: &[&str] = &[
        "\
local function f()
    print(1)
    return
end
f()",
        "\
print(1)
return",
        "\
local function f()
    return
end
f()",
        "\
local function f()
    print(1)
    return;
end
f()",
        "\
local f = function()
    local g = function()
        return
    end
    return g
end
f()",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RedundantReturn, PASS, FAIL).test_and_snapshot();
    }
}
