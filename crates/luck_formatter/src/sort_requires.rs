//! Sorts runs of `local NAME = require(...)` and
//! `local NAME = X:GetService(...)` statements by binding name.
//!
//! Works on the source lines of formatted text instead of the AST, so
//! comments and trivia on a line move with it. A run is two or more sortable
//! statements on consecutive lines, outside any `-- luck: format off` region.
//! A statement is sortable only if its lines hold nothing that can't move with
//! it. A blank line, a comment between two statements, a statement under
//! `luck: ignore`, a `<close>` binding, or any other statement ends a run, and
//! a run whose members read each other's bindings stays as written.
//!
//! [`crate::format`] only ever runs this on formatted text, so every line
//! ends in a line break and no `;` stands between two statements.

use core::mem;
use core::ops::Range;

use luck_ast::{
    Block, Expression, FunctionArgs, FunctionCall, LocalAssignment, Punctuated, Statement, Var,
};
use luck_token::{
    Comment, CommentKind, CommentPosition, LuaVersion, Span, has_line_break, index_to_offset,
    line_breaks, offset_to_index,
};

use crate::comments::{is_code_free, own_line_start};
use crate::directive::{Directive, format_off_regions};
use crate::format_block::has_blank_line;

struct SortableLine<'source> {
    /// Byte range of the statement's whole lines, from leading indentation
    /// through the trailing newline, so ranges swap cleanly.
    range: Range<usize>,
    /// The source text under `range`.
    text: &'source str,
    /// The binding name, used as the sort key.
    key: &'source str,
    /// The name the value reads: `require`, or the `GetService` receiver.
    callee: &'source str,
}

/// `source` with every run sorted, or `None` when there's no run to sort.
pub(crate) fn sort_requires_in_source(
    source: &str,
    block: &Block,
    comments: &[Comment],
    version: LuaVersion,
) -> Option<String> {
    let source_end = index_to_offset(source.len());
    let mut collector = RunCollector {
        source,
        version,
        comments,
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
    version: LuaVersion,
    comments: &'source [Comment],
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
            let is_blank_break = previous_end.is_some_and(|previous| {
                has_blank_line(self.source, previous, stmt_start, self.version)
            });

            let is_disabled = self
                .disabled_ranges
                .iter()
                .any(|range| stmt_start < range.end && stmt_end > range.start);

            let comments_above = self.comments_above(stmt_start);
            let has_comment_since_last_line = current_lines.last().is_some_and(|last_line| {
                comments_above
                    .iter()
                    .any(|comment| offset_to_index(comment.span.start) >= last_line.range.end)
            });

            if is_blank_break || is_disabled || has_comment_since_last_line {
                self.flush_run(&mut current_lines);
            }

            let is_ignored = comments_above.iter().any(|comment| {
                comment.position == CommentPosition::Leading
                    && Directive::parse(comment.span.text(self.source)) == Some(Directive::Ignore)
            });
            if !is_ignored
                && let Some(line) = sortable_line(self.source, self.comments, stmt, self.version)
            {
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

    /// Keeps `lines` as a run when it has two or more lines and sorting can't
    /// change what they read, then empties it.
    fn flush_run(&mut self, lines: &mut Vec<SortableLine<'source>>) {
        let has_dependency =
            lines.iter().any(|line| lines.iter().any(|other| other.key == line.callee));
        if lines.len() >= 2 && !has_dependency {
            self.runs.push(mem::take(lines));
        } else {
            lines.clear();
        }
    }

    /// The comments right above `stmt_start`, those only whitespace and `;`
    /// separate from it, nearest last.
    fn comments_above(&self, stmt_start: u32) -> &'source [Comment] {
        let end = self.comments.partition_point(|comment| comment.span.start < stmt_start);
        let mut first = end;
        let mut next_start = stmt_start;
        while let Some(comment) = first.checked_sub(1).and_then(|index| self.comments.get(index))
            && is_code_free(Span::new(comment.span.end, next_start).text(self.source))
        {
            first = first.saturating_sub(1);
            next_start = comment.span.start;
        }
        self.comments.get(first..end).unwrap_or_default()
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
/// in any dialect, whose whole lines can move as a unit.
fn sortable_line<'source>(
    source: &'source str,
    comments: &[Comment],
    stmt: &Statement,
    version: LuaVersion,
) -> Option<SortableLine<'source>> {
    let Statement::LocalAssignment(local) = stmt else {
        return None;
    };

    let key = single_binding_name(source, local)?;
    let exprs = local.exprs.as_ref()?;
    if exprs.len() != 1 {
        return None;
    }
    let callee = pure_require_callee(source, exprs.first()?)?;

    let start = offset_to_index(own_line_start(source, local.span.start)?);
    let end = movable_line_end(source, comments, local.span.end, version)?;
    let range = start..end;
    let text = source.get(range.clone())?;
    Some(SortableLine { range, text, key, callee })
}

/// The one name `local` binds, unless it's a `<close>` binding, whose
/// declaration order sets the order things close in.
fn single_binding_name<'source>(
    source: &'source str,
    local: &LocalAssignment,
) -> Option<&'source str> {
    if local.names.len() != 1 {
        return None;
    }
    let binding = local.names.first()?;
    if binding
        .attribute
        .as_deref()
        .is_some_and(|attribute| attribute.name.span.text(source) == "close")
    {
        return None;
    }
    Some(binding.name.span.text(source))
}

/// The name read by `require(STRING)` or `NAME:GetService(STRING)`, with or
/// without the parentheses.
///
/// Anything else, such as `require("a")()`, may have side effects that
/// sorting would reorder.
fn pure_require_callee<'source>(source: &'source str, expr: &Expression) -> Option<&'source str> {
    let Expression::FunctionCall(call) = expr else {
        return None;
    };
    let FunctionCall { callee: Expression::Var(Var::Name(callee)), method, args, .. } = &**call
    else {
        return None;
    };
    if !is_single_string_arg(args) {
        return None;
    }

    let is_pure = method.as_ref().map_or_else(
        || callee.span.text(source) == "require",
        |method_name| method_name.span.text(source) == "GetService",
    );
    is_pure.then(|| callee.span.text(source))
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

/// The end of the line `stmt_end` is on, through its line break, when all
/// that follows the statement there can move with it: whitespace, `;`, and
/// comments that stay on the line.
fn movable_line_end(
    source: &str,
    comments: &[Comment],
    stmt_end: u32,
    version: LuaVersion,
) -> Option<usize> {
    let first_index = comments.partition_point(|comment| comment.span.start < stmt_end);
    let mut cursor = stmt_end;
    for comment in comments.get(first_index..)? {
        let gap = Span::new(cursor, comment.span.start).text(source);
        if has_line_break(gap, version) {
            break;
        }
        if !is_code_free(gap) || comment.kind == CommentKind::MultiLineBlock {
            return None;
        }
        cursor = comment.span.end;
    }

    let cursor_index = offset_to_index(cursor);
    let rest = source.get(cursor_index..)?;
    let line_length =
        line_breaks(rest, version).next().map_or(rest.len(), |line_break| line_break.end);
    is_code_free(rest.get(..line_length)?).then(|| cursor_index.saturating_add(line_length))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sort_source(input: &str) -> String {
        let parsed = luck_parser::parse(input, LuaVersion::Lua54);
        assert!(parsed.errors.is_empty(), "parse: {:?}", parsed.errors);
        sort_requires_in_source(input, &parsed.block, &parsed.comments, LuaVersion::Lua54)
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
