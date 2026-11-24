use luck_ast::{AstKind, AstType, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags a statement that starts on the line where the previous one ends
/// (Luau's `SameLineStatement` lint).
///
/// Unlike Luau's lint, a `;` between them doesn't silence it: one statement
/// per line keeps diffs and stack traces clean either way. Disable the rule
/// if you prefer the dense style.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MultipleStatementsPerLine;

declare_lint_rule!(
    MultipleStatementsPerLine,
    Style,
    Warning,
    "multiple statements on the same line"
);

impl Rule for MultipleStatementsPerLine {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        for (previous_stmt, next_stmt) in block.stmts.iter().zip(block.stmts.iter().skip(1)) {
            let (previous, next) = (previous_stmt.span(), next_stmt.span());
            let lines = context.line_index();
            // `end` is exclusive, so step back one byte to stay on the line
            // where `previous` ends.
            if lines.line(previous.end.saturating_sub(1)) == lines.line(next.start) {
                context.diagnostic(
                    Finding::new(
                        "statement starts on the same line as the previous statement",
                        next,
                    )
                    .with_label(previous, "the previous statement")
                    .with_help("put each statement on its own line"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local a = 1
local b = 2",
        // Lua reads a lone CR as a line break.
        "local a = 1\rlocal b = 2",
    ];

    const FAIL: &[&str] = &[
        "local a = 1 local b = 2",
        "local a = 1; local b = 2",
        "\
do
  local a = 1 local b = 2
end",
        "local a = 1 local b = 2 local c = 3",
        "local function f() local x = 1 return x end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::MultipleStatementsPerLine, PASS, FAIL).test_and_snapshot();
    }
}
