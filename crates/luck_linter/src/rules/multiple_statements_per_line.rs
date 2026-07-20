use luck_ast::Statement;
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;
use luck_token::Span;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Luau lint #5: more than one statement starting on the same line.
///
/// Why we flag the semicolon-separated form too: some style guides
/// permit `a = 1; b = 2` as a deliberate dense format, but most
/// projects reserve a line per statement to keep diffs and stack
/// traces clean. Treating both the implicit and explicit separator
/// the same gives users a single, predictable rule - disable it if
/// you prefer the dense style.
pub struct MultipleStatementsPerLine;

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

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let source = ctx.source;
        let lines = LineIndex::new(source);
        let mut checker = MultiStmtChecker {
            diagnostics: Vec::new(),
            lines,
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

/// Maps byte positions to 1-based line numbers in O(log n) per query.
struct LineIndex {
    /// Byte offset at which each line starts. Line 1 starts at index 0.
    line_starts: Vec<u32>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0u32];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i as u32 + 1);
            }
        }
        Self {
            line_starts: starts,
        }
    }

    fn line_of(&self, byte_pos: u32) -> u32 {
        match self.line_starts.binary_search(&byte_pos) {
            Ok(idx) => idx as u32 + 1,
            Err(idx) => idx as u32,
        }
    }
}

struct MultiStmtChecker {
    diagnostics: Vec<LintDiagnostic>,
    lines: LineIndex,
}

impl MultiStmtChecker {
    fn check_block(&mut self, block: &Block) {
        let mut spans: Vec<Span> = Vec::with_capacity(block.stmts.len() + 1);
        for stmt in &block.stmts {
            // Skip empty statements (bare `;`) - they don't carry user
            // intent, just punctuation.
            if matches!(stmt, Statement::EmptyStatement(_)) {
                continue;
            }
            spans.push(stmt.span());
        }
        if let Some(last) = &block.last_stmt {
            spans.push(last.span());
        }

        for window in spans.windows(2) {
            let prev = window[0];
            let next = window[1];
            // Compare the line of the last source byte of `prev` to the
            // first source byte of `next`. `end` is exclusive, so
            // subtract one to land on the actual last byte.
            let prev_end_byte = prev.end.saturating_sub(1);
            let prev_line = self.lines.line_of(prev_end_byte);
            let next_line = self.lines.line_of(next.start);
            if prev_line == next_line {
                self.diagnostics.push(LintDiagnostic::new("multiple_statements_per_line", format!(
                        "statement starts on the same line as the previous statement (line {prev_line})"
                    ), next).with_help("put each statement on its own line".to_string()));
            }
        }
    }
}

impl<'ast> Visitor<'ast> for MultiStmtChecker {
    fn visit_block(&mut self, block: &'ast Block) {
        self.check_block(block);
        self.walk_block(block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&MultipleStatementsPerLine, source, LuaVersion::Lua54)
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
        // Each adjacency past the first fires once, so a-b and b-c.
        let diags = run("local a = 1 local b = 2 local c = 3");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn flags_return_on_same_line() {
        let diags = run("local function f() local x = 1 return x end");
        assert!(!diags.is_empty(), "got: {diags:?}");
    }
}
