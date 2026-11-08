//! Sorts runs of `local NAME = require(...)` and
//! `local NAME = X:GetService(...)` statements by binding name.
//!
//! Works on source lines instead of the AST, so comments and trivia on a line
//! move with it. A run is two or more sortable statements on consecutive
//! lines, outside any `-- luck: format off` region. A blank line, a comment
//! line, or any other statement but `;` ends it.

use core::mem;
use core::ops::Range;

use luck_ast::{Block, Expression, FunctionArgs, LocalAssignment, Punctuated, Statement, Var};
use luck_token::{Comment, Span, index_to_offset, offset_to_index};

use crate::directive::format_off_regions;
use crate::format_block::has_blank_line;

struct SortableLine<'source> {
    /// Byte range of the statement's whole lines, from leading indentation
    /// through the trailing newline, so ranges swap cleanly.
    range: Range<usize>,
    /// The source text under `range`.
    text: &'source str,
    /// The binding name, used as the sort key.
    key: &'source str,
}

/// `source` with every run sorted, or `None` when there's no run to sort.
pub(crate) fn sort_requires_in_source(
    source: &str,
    block: &Block,
    comments: &[Comment],
) -> Option<String> {
    let source_end = index_to_offset(source.len());
    let mut collector = RunCollector {
        source,
        disabled_ranges: format_off_regions(comments, source)
            .into_iter()
            .map(|(off, on)| off.start..on.map_or(source_end, |close| close.end))
            .collect(),
        runs: Vec::new(),
    };
    collector.collect_block(block);
    let runs = collector.runs;
    if runs.is_empty() {
        return None;
    }

    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;

    for run in &runs {
        let [first, .., last] = run.as_slice() else {
            continue;
        };
        if let Some(gap) = source.get(cursor..first.range.start) {
            output.push_str(gap);
        }

        let mut sorted_lines: Vec<&SortableLine<'_>> = run.iter().collect();
        sorted_lines.sort_by_key(|line| line.key);

        for line in sorted_lines {
            output.push_str(line.text);
        }

        cursor = last.range.end;
    }

    if let Some(tail) = source.get(cursor..) {
        output.push_str(tail);
    }

    Some(output)
}

/// Finds the sortable runs in a block and the blocks nested in it, in
/// source order.
struct RunCollector<'source> {
    source: &'source str,
    disabled_ranges: Vec<Range<u32>>,
    runs: Vec<Vec<SortableLine<'source>>>,
}

impl<'source> RunCollector<'source> {
    /// Collects runs from `block` and the blocks nested in it.
    ///
    /// A function literal is searched only when it's assigned directly, so
    /// one passed as a call argument or held in a table isn't.
    fn collect_block(&mut self, block: &Block) {
        let mut current_lines: Vec<SortableLine<'source>> = Vec::new();
        let mut previous_end: Option<u32> = None;

        for stmt in &block.stmts {
            let Span { start: stmt_start, end: stmt_end } = stmt.span();

            // Counts newlines, so a comment line breaks the run as well.
            let is_blank_break = previous_end
                .is_some_and(|previous| has_blank_line(self.source, previous, stmt_start));

            let is_disabled = self
                .disabled_ranges
                .iter()
                .any(|range| stmt_start < range.end && stmt_end > range.start);

            if is_blank_break || is_disabled {
                self.flush_run(&mut current_lines);
            }

            // The formatter drops `;` in this same pass, so ending the run
            // here would leave the sort to the second pass and break
            // idempotency.
            if matches!(stmt, Statement::EmptyStatement(_)) {
                previous_end = Some(stmt_end);
                continue;
            }

            if let Some(line) = sortable_line(self.source, stmt) {
                if !is_disabled {
                    current_lines.push(line);
                }
            } else {
                self.flush_run(&mut current_lines);
            }

            self.collect_nested_blocks(stmt);

            previous_end = Some(stmt_end);
        }

        self.flush_run(&mut current_lines);
    }

    /// Keeps `lines` as a run when it has two or more lines, then
    /// empties it.
    ///
    /// Requires that share a line get the same line range, and sorting would
    /// emit that line once per member, so such a run isn't kept.
    fn flush_run(&mut self, lines: &mut Vec<SortableLine<'source>>) {
        let has_shared_line = lines
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if right.range.start < left.range.end));
        if lines.len() >= 2 && !has_shared_line {
            self.runs.push(mem::take(lines));
        } else {
            lines.clear();
        }
    }

    fn collect_nested_blocks(&mut self, stmt: &Statement) {
        match stmt {
            Statement::DoBlock(node) => self.collect_block(&node.block),
            Statement::WhileLoop(node) => self.collect_block(&node.block),
            Statement::RepeatLoop(node) => self.collect_block(&node.block),
            Statement::NumericFor(node) => self.collect_block(&node.block),
            Statement::GenericFor(node) => self.collect_block(&node.block),
            Statement::IfStatement(node) => {
                self.collect_block(&node.block);
                for clause in &node.elseif_clauses {
                    self.collect_block(&clause.block);
                }
                if let Some(else_clause) = &node.else_clause {
                    self.collect_block(&else_clause.block);
                }
            }
            Statement::FunctionDecl(node) => self.collect_block(&node.body.block),
            Statement::LocalFunction(node) => self.collect_block(&node.body.block),
            // Lua 5.5
            Statement::GlobalFunction(node) => self.collect_block(&node.body.block),
            Statement::LocalAssignment(local) => {
                if let Some(exprs) = &local.exprs {
                    self.collect_function_values(exprs);
                }
            }
            Statement::Assignment(node) => self.collect_function_values(&node.values),
            // Luau
            Statement::CompoundAssignment(node) => self.collect_function_value(&node.expr),
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                if let Some(exprs) = &global.exprs {
                    self.collect_function_values(exprs);
                }
            }
            Statement::FunctionCall(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }

    fn collect_function_values(&mut self, exprs: &Punctuated<Expression>) {
        for expr in exprs.iter() {
            self.collect_function_value(expr);
        }
    }

    fn collect_function_value(&mut self, expr: &Expression) {
        if let Expression::FunctionDef(function_def) = expr {
            self.collect_block(&function_def.body.block);
        }
    }
}

/// Matches a `local` with one name bound to one require or `GetService` call,
/// in any dialect.
fn sortable_line<'source>(source: &'source str, stmt: &Statement) -> Option<SortableLine<'source>> {
    let Statement::LocalAssignment(local) = stmt else {
        return None;
    };

    let key = single_binding_name(source, local)?;
    let exprs = local.exprs.as_ref()?;
    if exprs.len() != 1 {
        return None;
    }
    let value = exprs.first()?;
    if !is_pure_require_call(source, value) {
        return None;
    }

    let range = line_range(source, local.span);
    let text = source.get(range.clone())?;
    Some(SortableLine { range, text, key })
}

fn single_binding_name<'source>(
    source: &'source str,
    local: &LocalAssignment,
) -> Option<&'source str> {
    if local.names.len() != 1 {
        return None;
    }
    Some(local.names.first()?.name.span.text(source))
}

/// Matches `require(STRING)` and `NAME:GetService(STRING)`, with or without
/// the parentheses.
///
/// Anything else, such as `require("a")()`, may have side effects that
/// sorting would reorder.
fn is_pure_require_call(source: &str, expr: &Expression) -> bool {
    let Expression::FunctionCall(call) = expr else {
        return false;
    };

    if matches!(call.callee, Expression::FunctionCall(_)) {
        return false;
    }

    if !is_single_string_arg(&call.args) {
        return false;
    }

    if call.method.is_none()
        && let Expression::Var(Var::Name(token)) = &call.callee
    {
        return token.span.text(source) == "require";
    }

    if let Some(method_name) = &call.method
        && let Expression::Var(Var::Name(_)) = &call.callee
    {
        return method_name.span.text(source) == "GetService";
    }

    false
}

fn is_single_string_arg(call_args: &FunctionArgs) -> bool {
    match call_args {
        FunctionArgs::Parenthesized { args, .. } => {
            args.len() == 1 && matches!(args.first(), Some(Expression::StringLiteral(_)))
        }
        FunctionArgs::StringLiteral(_) => true,
        FunctionArgs::TableConstructor(_) => false,
    }
}

/// Widens `stmt_span` to the whole source lines it occupies, including the
/// trailing newline.
fn line_range(source: &str, stmt_span: Span) -> Range<usize> {
    let line_start = Span::new(0, stmt_span.start)
        .text(source)
        .trim_end_matches(|character: char| character != '\n')
        .len();

    let stmt_end = offset_to_index(stmt_span.end);
    let line_remainder = source
        .get(stmt_end..)
        .and_then(|following| following.split_inclusive('\n').next())
        .unwrap_or_default();

    line_start..stmt_end.saturating_add(line_remainder.len())
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;

    fn sort_source(input: &str) -> String {
        let parsed = luck_parser::parse(input, LuaVersion::Lua54);
        assert!(parsed.errors.is_empty(), "parse: {:?}", parsed.errors);
        sort_requires_in_source(input, &parsed.block, &parsed.comments)
            .unwrap_or_else(|| input.to_owned())
    }

    #[test]
    fn sorts_simple_run() {
        let input = "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\n";
        let expected = "local alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\nlocal zeta = require(\"zeta\")\n";
        assert_eq!(sort_source(input), expected);
    }

    #[test]
    fn blank_line_breaks_run() {
        let input = "local zeta = require(\"zeta\")\n\nlocal alpha = require(\"alpha\")\n";
        // The blank line leaves two one-statement runs, too short to sort.
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn non_require_breaks_run() {
        let input =
            "local zeta = require(\"zeta\")\nlocal x = 1\nlocal alpha = require(\"alpha\")\n";
        // The `local x = 1` line splits the run; no reordering.
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn multi_binding_is_unsortable() {
        let input =
            "local zeta = require(\"zeta\")\nlocal a, b = 1, 2\nlocal alpha = require(\"alpha\")\n";
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn side_effect_require_is_unsortable() {
        let input = "local zeta = require(\"zeta\")()\nlocal alpha = require(\"alpha\")()\n";
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn format_off_region_is_preserved() {
        let input = "-- luck: format off\nlocal zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n-- luck: format on\n";
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn mixed_require_and_get_service() {
        let input = "local zeta = require(\"zeta\")\nlocal alpha = game:GetService(\"Players\")\n";
        let expected =
            "local alpha = game:GetService(\"Players\")\nlocal zeta = require(\"zeta\")\n";
        assert_eq!(sort_source(input), expected);
    }

    #[test]
    fn nested_function_body_run_sorts() {
        let input = "local function f()\n\tlocal zeta = require(\"zeta\")\n\tlocal alpha = require(\"alpha\")\nend\n";
        let expected = "local function f()\n\tlocal alpha = require(\"alpha\")\n\tlocal zeta = require(\"zeta\")\nend\n";
        assert_eq!(sort_source(input), expected);
    }
}
