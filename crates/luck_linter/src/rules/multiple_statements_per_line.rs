use luck_ast::{Block, Statement, Visitor};
use luck_token::{Span, index_to_offset};

use crate::diagnostic::{Category, Finding, Severity};
use crate::rule::{LintContext, Rule, WholeRule};

/// Flags a statement that starts on the line where the previous one ends
/// (Luau's `SameLineStatement` lint).
///
/// Unlike Luau's lint, a `;` between them doesn't silence it: one statement
/// per line keeps diffs and stack traces clean either way. Disable the rule
/// if you prefer the dense style.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MultipleStatementsPerLine;

impl Rule for MultipleStatementsPerLine {
    fn name(&self) -> &'static str {
        "multiple_statements_per_line"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "multiple statements on the same line"
    }
}

impl WholeRule for MultipleStatementsPerLine {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut checker =
            SameLineChecker { findings: Vec::new(), lines: LineIndex::new(context.source) };
        checker.visit_block(context.block);
        checker.findings
    }
}

/// Maps byte positions to 1-based line numbers in O(log n) per query.
struct LineIndex {
    /// Byte offset of each `\n`, in source order.
    line_breaks: Vec<u32>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let line_breaks =
            source.match_indices('\n').map(|(index, _)| index_to_offset(index)).collect();
        Self { line_breaks }
    }

    fn line_of(&self, offset: u32) -> usize {
        self.line_breaks.partition_point(|&line_break| line_break < offset).saturating_add(1)
    }
}

struct SameLineChecker {
    findings: Vec<Finding>,
    lines: LineIndex,
}

impl SameLineChecker {
    fn check_block(&mut self, block: &Block) {
        let mut spans: Vec<Span> = Vec::with_capacity(block.stmts.len().saturating_add(1));
        for stmt in &block.stmts {
            // A bare `;` is a separator, not a statement.
            if matches!(stmt, Statement::EmptyStatement(_)) {
                continue;
            }
            spans.push(stmt.span());
        }
        if let Some(last) = &block.last_stmt {
            spans.push(last.span());
        }

        for [previous, next] in spans.array_windows() {
            // `end` is exclusive, so step back one byte to stay on the line
            // where `previous` ends.
            let previous_line = self.lines.line_of(previous.end.saturating_sub(1));
            if previous_line == self.lines.line_of(next.start) {
                self.findings.push(
                    Finding::new(
                        format!(
                            "statement starts on the same line as the previous statement (line \
                             {previous_line})"
                        ),
                        *next,
                    )
                    .with_help("put each statement on its own line"),
                );
            }
        }
    }
}

impl<'ast> Visitor<'ast> for SameLineChecker {
    fn visit_block(&mut self, block: &'ast Block) {
        self.check_block(block);
        self.walk_block(block);
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
}
