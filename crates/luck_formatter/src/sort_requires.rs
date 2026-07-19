//! Sort consecutive `local NAME = require(...)` (or `:GetService(...)`)
//! statements alphabetically.
//!
//! Operates at the source level by rewriting byte ranges rather than mutating
//! the AST - preserves comments, blank lines, and all trivia inside each line.
//! A "group" is a run of consecutive sortable statements with no intervening
//! blank line, non-sortable statement, or `-- luck: format off` region.

use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_ast::shared::{Block, FunctionBody, Punctuated};
use luck_ast::stmt::{LocalAssignment, Statement};
use luck_token::comment::Comment;

/// Returned spans are byte offsets into the original source.
struct SortableLine<'a> {
    /// Byte range covering the whole line, including leading indentation and
    /// the trailing newline, so that swapping ranges produces clean output.
    range: std::ops::Range<usize>,
    /// Sort key - the binding name.
    key: &'a str,
}

pub fn sort_requires_in_source(source: &str, block: &Block, comments: &[Comment]) -> String {
    let disabled = build_disabled_ranges(comments, source);
    let mut groups = Vec::new();
    collect_groups_in_block(source, block, &disabled, &mut groups);

    if groups.is_empty() {
        return source.to_string();
    }

    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;

    for group in &groups {
        // Two requires sharing a source line get the same expanded line
        // range; sorting would emit that line once per member, duplicating
        // code. Leave any group with overlapping ranges untouched.
        let has_overlap = group
            .windows(2)
            .any(|pair| pair[1].range.start < pair[0].range.end);
        if has_overlap {
            continue;
        }

        let group_start = group[0].range.start;
        if cursor < group_start {
            output.push_str(&source[cursor..group_start]);
        }

        let mut sorted: Vec<&SortableLine> = group.iter().collect();
        sorted.sort_by(|a, b| a.key.cmp(b.key));

        for line in sorted {
            output.push_str(&source[line.range.clone()]);
        }

        cursor = group.last().expect("group non-empty").range.end;
    }

    if cursor < source.len() {
        output.push_str(&source[cursor..]);
    }

    output
}

/// Walk every block in the program, collecting sortable groups along the way.
fn collect_groups_in_block<'a>(
    source: &'a str,
    block: &Block,
    disabled: &[(u32, u32)],
    out: &mut Vec<Vec<SortableLine<'a>>>,
) {
    let mut current: Vec<SortableLine<'a>> = Vec::new();
    let mut prev_end: Option<u32> = None;

    for stmt in &block.stmts {
        let stmt_start = stmt.span().start;
        let stmt_end = stmt.span().end;

        // Blank line breaks the run.
        let blank_break =
            prev_end.is_some_and(|prev| has_blank_line_between(source, prev, stmt_start));

        // Format-off region anywhere in the statement breaks the run.
        let in_disabled = disabled
            .iter()
            .any(|&(start, end)| stmt_start < end && stmt_end > start);

        if blank_break || in_disabled {
            flush_group(&mut current, out);
        }

        // `;` carries no content and the formatter drops it on the same
        // pass - treating it as a group break made sorting kick in only on
        // the SECOND format run, breaking idempotency.
        if matches!(stmt, Statement::EmptyStatement(_)) {
            prev_end = Some(stmt_end);
            continue;
        }

        if let Some(line) = sortable_line(source, stmt) {
            if !in_disabled {
                current.push(line);
            }
        } else {
            flush_group(&mut current, out);
        }

        // Recurse into nested blocks so top-of-function-body groups also sort.
        walk_nested_blocks(source, stmt, disabled, out);

        prev_end = Some(stmt_end);
    }

    flush_group(&mut current, out);
}

fn flush_group<'a>(current: &mut Vec<SortableLine<'a>>, out: &mut Vec<Vec<SortableLine<'a>>>) {
    if current.len() >= 2 {
        out.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn walk_nested_blocks<'a>(
    source: &'a str,
    stmt: &Statement,
    disabled: &[(u32, u32)],
    out: &mut Vec<Vec<SortableLine<'a>>>,
) {
    match stmt {
        Statement::DoBlock(node) => collect_groups_in_block(source, &node.block, disabled, out),
        Statement::WhileLoop(node) => collect_groups_in_block(source, &node.block, disabled, out),
        Statement::RepeatLoop(node) => collect_groups_in_block(source, &node.block, disabled, out),
        Statement::NumericFor(node) => collect_groups_in_block(source, &node.block, disabled, out),
        Statement::GenericFor(node) => collect_groups_in_block(source, &node.block, disabled, out),
        Statement::IfStatement(node) => {
            collect_groups_in_block(source, &node.block, disabled, out);
            for clause in &node.elseif_clauses {
                collect_groups_in_block(source, &clause.block, disabled, out);
            }
            if let Some(else_clause) = &node.else_clause {
                collect_groups_in_block(source, &else_clause.block, disabled, out);
            }
        }
        Statement::FunctionDecl(node) => walk_body(source, &node.body, disabled, out),
        Statement::LocalFunction(node) => walk_body(source, &node.body, disabled, out),
        Statement::GlobalFunction(node) => walk_body(source, &node.body, disabled, out),
        Statement::LocalAssignment(local) => {
            if let Some((_, exprs)) = &local.equal_and_exprs {
                walk_exprs(source, exprs, disabled, out);
            }
        }
        Statement::Assignment(node) => walk_exprs(source, &node.values, disabled, out),
        Statement::FunctionCall(call) => walk_call(source, &call.call, disabled, out),
        Statement::CompoundAssignment(node) => walk_expr(source, &node.expr, disabled, out),
        Statement::GlobalDeclaration(global) => {
            if let Some((_, exprs)) = &global.equal_and_exprs {
                walk_exprs(source, exprs, disabled, out);
            }
        }
        Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => {}
    }
}

fn walk_body<'a>(
    source: &'a str,
    body: &FunctionBody,
    disabled: &[(u32, u32)],
    out: &mut Vec<Vec<SortableLine<'a>>>,
) {
    collect_groups_in_block(source, &body.block, disabled, out);
}

fn walk_exprs<'a>(
    source: &'a str,
    exprs: &Punctuated<Expression>,
    disabled: &[(u32, u32)],
    out: &mut Vec<Vec<SortableLine<'a>>>,
) {
    for expr in exprs.iter() {
        walk_expr(source, expr, disabled, out);
    }
}

fn walk_expr<'a>(
    source: &'a str,
    expr: &Expression,
    disabled: &[(u32, u32)],
    out: &mut Vec<Vec<SortableLine<'a>>>,
) {
    if let Expression::FunctionDef(def) = expr {
        walk_body(source, &def.body, disabled, out);
    }
}

fn walk_call<'a>(
    source: &'a str,
    _call: &FunctionCall,
    _disabled: &[(u32, u32)],
    _out: &mut Vec<Vec<SortableLine<'a>>>,
) {
    let _ = source;
    // No nested blocks reachable through a bare call statement at the top level.
}

/// Recognize one of:
///   `local NAME = require(<string>)`
///   `local NAME = require("path")`
///   `local NAME = game:GetService("Foo")` (Roblox / Luau compatibility)
/// Multi-name (`local a, b = ...`) and chained calls are rejected.
fn sortable_line<'a>(source: &'a str, stmt: &Statement) -> Option<SortableLine<'a>> {
    let Statement::LocalAssignment(local) = stmt else {
        return None;
    };

    let key = single_binding_name(source, local)?;
    let (_, exprs) = local.equal_and_exprs.as_ref()?;
    if exprs.len() != 1 {
        return None;
    }
    let value = exprs.first()?;
    if !is_pure_require_call(source, value) {
        return None;
    }

    Some(SortableLine {
        range: line_range(source, local.span.start as usize, local.span.end as usize),
        key,
    })
}

fn single_binding_name<'a>(source: &'a str, local: &LocalAssignment) -> Option<&'a str> {
    if local.names.len() != 1 {
        return None;
    }
    let name = &local.names.first()?.name;
    Some(&source[name.span.start as usize..name.span.end as usize])
}

/// Recognize the require/GetService callee patterns. Anything else (including
/// `require("a")()` chains) is rejected because it may have side effects we
/// must not reorder.
fn is_pure_require_call(source: &str, expr: &Expression) -> bool {
    let Expression::FunctionCall(call) = expr else {
        return false;
    };

    // Chained call like `require("x")()` - outermost call's callee is itself
    // a FunctionCall. Reject so we never reorder side effects.
    if matches!(call.callee, Expression::FunctionCall(_)) {
        return false;
    }

    // Require args must be a single string literal (parenthesized or bare).
    if !is_single_string_arg(&call.args) {
        return false;
    }

    // Plain `require(...)`
    if call.method.is_none()
        && let Expression::Var(var) = &call.callee
        && let Var::Name(token) = var.as_ref()
    {
        let name = &source[token.span.start as usize..token.span.end as usize];
        return name == "require";
    }

    // `game:GetService("Foo")` - method call on a Name var.
    if let Some((_, method_name)) = &call.method
        && let Expression::Var(var) = &call.callee
        && let Var::Name(_) = var.as_ref()
    {
        let method = &source[method_name.span.start as usize..method_name.span.end as usize];
        return method == "GetService";
    }

    false
}

fn is_single_string_arg(args: &FunctionArgs) -> bool {
    match args {
        FunctionArgs::Parenthesized { args, .. } => {
            args.len() == 1 && matches!(args.first(), Some(Expression::StringLiteral(_)))
        }
        FunctionArgs::StringLiteral(_) => true,
        FunctionArgs::TableConstructor(_) => false,
    }
}

/// Expand the statement byte range to cover the full source lines it occupies,
/// including the trailing newline. We do not extend over a blank line.
fn line_range(source: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let bytes = source.as_bytes();

    let mut line_start = start;
    while line_start > 0 && bytes[line_start - 1] != b'\n' {
        line_start -= 1;
    }

    let mut line_end = end;
    while line_end < bytes.len() && bytes[line_end] != b'\n' {
        line_end += 1;
    }
    if line_end < bytes.len() {
        line_end += 1;
    }

    line_start..line_end
}

/// Two or more newlines between `prev_end` and `next_start` means a blank line.
fn has_blank_line_between(source: &str, prev_end: u32, next_start: u32) -> bool {
    let start = prev_end as usize;
    let end = next_start as usize;
    if start >= end || end > source.len() {
        return false;
    }
    source[start..end].bytes().filter(|&b| b == b'\n').count() >= 2
}

/// Pair `-- luck: format off` with the next `-- luck: format on` (or EOF).
fn build_disabled_ranges(comments: &[Comment], source: &str) -> Vec<(u32, u32)> {
    let mut ranges = Vec::new();
    let mut off_start: Option<u32> = None;

    for comment in comments {
        let text = &source[comment.span.start as usize..comment.span.end as usize];
        let trimmed = text.trim_start_matches('-').trim();
        if trimmed.starts_with("luck: format off") {
            if off_start.is_none() {
                off_start = Some(comment.span.start);
            }
        } else if trimmed.starts_with("luck: format on")
            && let Some(start) = off_start.take()
        {
            ranges.push((start, comment.span.end));
        }
    }

    if let Some(start) = off_start {
        ranges.push((start, source.len() as u32));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(input: &str) -> String {
        let parsed = luck_parser::parse(input, LuaVersion::Lua54);
        assert!(parsed.errors.is_empty(), "parse: {:?}", parsed.errors);
        sort_requires_in_source(input, &parsed.block, &parsed.comments)
    }

    #[test]
    fn sorts_simple_group() {
        let input = "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\n";
        let expected = "local alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\nlocal zeta = require(\"zeta\")\n";
        assert_eq!(run(input), expected);
    }

    #[test]
    fn blank_line_breaks_group() {
        let input = "local zeta = require(\"zeta\")\n\nlocal alpha = require(\"alpha\")\n";
        // Blank line splits two single-line "groups" - neither has 2 entries,
        // so nothing is reordered.
        assert_eq!(run(input), input);
    }

    #[test]
    fn non_require_breaks_group() {
        let input =
            "local zeta = require(\"zeta\")\nlocal x = 1\nlocal alpha = require(\"alpha\")\n";
        // The `local x = 1` line splits the run; no reordering.
        assert_eq!(run(input), input);
    }

    #[test]
    fn multi_binding_is_unsortable() {
        let input =
            "local zeta = require(\"zeta\")\nlocal a, b = 1, 2\nlocal alpha = require(\"alpha\")\n";
        assert_eq!(run(input), input);
    }

    #[test]
    fn side_effect_require_is_unsortable() {
        let input = "local zeta = require(\"zeta\")()\nlocal alpha = require(\"alpha\")()\n";
        assert_eq!(run(input), input);
    }

    #[test]
    fn format_off_region_is_preserved() {
        let input = "-- luck: format off\nlocal zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n-- luck: format on\n";
        assert_eq!(run(input), input);
    }

    #[test]
    fn mixed_require_and_get_service() {
        let input = "local zeta = require(\"zeta\")\nlocal alpha = game:GetService(\"Players\")\n";
        let expected =
            "local alpha = game:GetService(\"Players\")\nlocal zeta = require(\"zeta\")\n";
        assert_eq!(run(input), expected);
    }

    #[test]
    fn nested_function_body_group_sorts() {
        let input = "local function f()\n\tlocal zeta = require(\"zeta\")\n\tlocal alpha = require(\"alpha\")\nend\n";
        let expected = "local function f()\n\tlocal alpha = require(\"alpha\")\n\tlocal zeta = require(\"zeta\")\nend\n";
        assert_eq!(run(input), expected);
    }
}
