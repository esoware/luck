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
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::{labeled, run_rule};
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::MultipleStatementsPerLine, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_adjacent_locals_one_line() {
        let source = "local a = 1 local b = 2";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(labeled(&diags[0], source), ["local a = 1"]);
    }

    #[test]
    fn ignores_separate_lines() {
        let diags = run("\
local a = 1
local b = 2");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_semicolon_separated() {
        let diags = run("local a = 1; local b = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_inside_nested_block() {
        let diags = run("\
do
  local a = 1 local b = 2
end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_three_on_one_line() {
        // One diagnostic per adjacent pair: a-b and b-c.
        let diags = run("local a = 1 local b = 2 local c = 3");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn flags_return_on_same_line() {
        let diags = run("local function f() local x = 1 return x end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_cr_only_line_breaks() {
        let diags = run("local a = 1\rlocal b = 2");
        assert!(diags.is_empty(), "Lua reads a lone CR as a line break: {diags:?}");
    }
}
