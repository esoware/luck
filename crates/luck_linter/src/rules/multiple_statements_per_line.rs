use luck_ast::{LastStatement, Statement};
use luck_token::{LuaVersion, Span, index_to_offset, line_breaks};

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
        let lines = LineIndex::new(context.source, context.semantic.version);
        let mut findings = Vec::new();
        let mut spans: Vec<Span> = Vec::new();
        for block in context.blocks {
            // A bare `;` is a separator, not a statement.
            let stmt_spans = block
                .stmts
                .iter()
                .filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_)))
                .map(Statement::span);
            spans.clear();
            spans.extend(stmt_spans.chain(block.last_stmt.as_deref().map(LastStatement::span)));

            for [previous, next] in spans.array_windows() {
                // `end` is exclusive, so step back one byte to stay on the
                // line where `previous` ends.
                let previous_line = lines.line_of(previous.end.saturating_sub(1));
                if previous_line == lines.line_of(next.start) {
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

/// Maps byte positions to 1-based line numbers in O(log n) per query.
struct LineIndex {
    /// Byte offset where each line after the first starts, in source order.
    line_starts: Vec<u32>,
}

impl LineIndex {
    fn new(source: &str, version: LuaVersion) -> Self {
        let line_starts = line_breaks(source, version)
            .map(|line_break| index_to_offset(line_break.end))
            .collect();
        Self { line_starts }
    }

    fn line_of(&self, offset: u32) -> usize {
        self.line_starts.partition_point(|&line_start| line_start <= offset).saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
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
        let diags = run("local a = 1\nlocal b = 2");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_semicolon_separated() {
        let diags = run("local a = 1; local b = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_inside_nested_block() {
        let diags = run("do\n  local a = 1 local b = 2\nend");
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
