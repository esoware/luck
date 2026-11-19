use luck_ast::{GetSpan as _, Statement};
use luck_token::Span;

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags a statement that starts on the line where the previous one ends
/// (Luau's `SameLineStatement` lint).
///
/// Unlike Luau's lint, a `;` between them doesn't silence it: one statement
/// per line keeps diffs and stack traces clean either way. Disable the rule
/// if you prefer the dense style.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MultipleStatementsPerLine;

impl_rule!(
    MultipleStatementsPerLine,
    "multiple_statements_per_line",
    Style,
    Warning,
    "multiple statements on the same line"
);

impl WholeRule for MultipleStatementsPerLine {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let lines = context.line_index();
        let mut findings = Vec::new();
        let mut spans: Vec<Span> = Vec::new();
        for block in context.nodes.blocks() {
            spans.clear();
            spans.extend(block.stmts.iter().map(Statement::span));

            for (previous, next) in spans.iter().zip(spans.iter().skip(1)) {
                // `end` is exclusive, so step back one byte to stay on the
                // line where `previous` ends.
                let previous_line = lines.line(previous.end.saturating_sub(1));
                if previous_line == lines.line(next.start) {
                    findings.push(
                        Finding::new(
                            format!(
                                "statement starts on the same line as the previous statement \
                                 (line {previous_line})"
                            ),
                            *next,
                        )
                        .with_help("put each statement on its own line"),
                    );
                }
            }
        }
        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&MultipleStatementsPerLine, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_adjacent_locals_one_line() {
        let diags = run("local a = 1 local b = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
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
