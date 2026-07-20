//! Block emission: the statement loop that threads the comment/verbatim
//! protocol and blank-line preservation between statements.
//!
//! Per statement the loop: (1) emits leading comments, learning whether a
//! `luck: ignore` directive applies; (2) emits the `(`-guard when needed;
//! (3) emits the statement verbatim (ignore / `format off` regions) or via
//! `Format`; (4) emits trailing comments. Separators between statements are
//! blank-line aware.

use luck_ast::shared::Block;
use luck_ast::stmt::{LastStatement, Statement};

use crate::ir::*;

impl Format for Block {
    fn fmt(&self, f: &mut Formatter) {
        // Carries the previous emitted item's end offset and whether it wants
        // a blank line around it (used only on the synthetic, source-less
        // path where offsets can't be scanned).
        let mut previous: Option<(u32, bool)> = None;

        let preserve_edge_blanks =
            f.options.block_newline_gaps == crate::BlockNewlineGaps::Preserve;
        if preserve_edge_blanks {
            if let Some(first_start) = first_item_start(self) {
                // A leading comment moves the measurable gap to its own start
                let anchor = f
                    .comments
                    .peek_next_start()
                    .filter(|start| *start < first_start)
                    .unwrap_or(first_start);
                let has_blank = f
                    .comments
                    .source_text()
                    .is_some_and(|source| has_leading_edge_blank(source, anchor));
                if has_blank {
                    crate::write!(f, [empty_line()]);
                }
            }
        }

        for stmt in &self.stmts {
            // A bare `;` and a parse-error placeholder produce no output and
            // must not open a separator gap.
            if matches!(stmt, Statement::EmptyStatement(_) | Statement::Error(_)) {
                continue;
            }

            let start = stmt.span().start;
            let end = stmt.span().end;
            let wants_blank = stmt_wants_surrounding_blank(stmt);

            if let Some((previous_end, previous_wants)) = previous {
                emit_separator(f, previous_end, start, previous_wants || wants_blank);
            }

            let is_ignored = f.emit_leading_comments(start);

            // A `(`-starting statement after another re-parses as a chained
            // call (`f()` + `(g)()` -> `f()(g)()`) - a silent semantics change.
            // Prefix `;` exactly like the compact printer.
            if previous.is_some() && luck_ast::query::stmt_starts_with_paren(stmt) {
                crate::write!(f, [token(";")]);
            }

            // Format-selection: statements outside the requested range stay
            // verbatim so an editor edit touches only the selection.
            let is_outside_range = f
                .format_range
                .as_ref()
                .is_some_and(|range| end <= range.start || start >= range.end);

            emit_verbatim_or(f, start, end, is_ignored || is_outside_range, |f| {
                stmt.fmt(f)
            });
            f.emit_trailing_comments(start, end);

            previous = Some((end, wants_blank));
        }

        if let Some(last) = &self.last_stmt {
            // Nothing to print for a recovery placeholder.
            if !matches!(last.as_ref(), LastStatement::Error(_)) {
                let start = last.span().start;
                let end = last.span().end;

                if let Some((previous_end, previous_wants)) = previous {
                    // A last statement (return/break/continue) never wants a blank
                    // line of its own, so only the previous item's preference counts.
                    emit_separator(f, previous_end, start, previous_wants);
                }

                let is_ignored = f.emit_leading_comments(start);
                let is_outside_range = f
                    .format_range
                    .as_ref()
                    .is_some_and(|range| end <= range.start || start >= range.end);
                emit_verbatim_or(f, start, end, is_ignored || is_outside_range, |f| {
                    last.fmt(f)
                });
                f.emit_trailing_comments(start, end);
                previous = Some((end, false));
            }
        }

        if preserve_edge_blanks {
            if let Some((last_end, _)) = previous {
                let has_blank = f
                    .comments
                    .source_text()
                    .is_some_and(|source| has_trailing_edge_blank(source, last_end));
                if has_blank {
                    crate::write!(f, [empty_line()]);
                }
            }
        }
    }
}

/// Start offset of the first item that will produce output.
pub(crate) fn first_item_start(block: &Block) -> Option<u32> {
    block
        .stmts
        .iter()
        .find(|stmt| !matches!(stmt, Statement::EmptyStatement(_) | Statement::Error(_)))
        .map(|stmt| stmt.span().start)
        .or_else(|| {
            block.last_stmt.as_ref().and_then(|last| {
                if matches!(last.as_ref(), LastStatement::Error(_)) {
                    None
                } else {
                    Some(last.span().start)
                }
            })
        })
}

/// Whether the whitespace run immediately before `anchor` contains a blank
/// line (the gap between a block opener and its first statement).
fn has_leading_edge_blank(source: &str, anchor: u32) -> bool {
    let mut newlines = 0;
    for byte in source[..(anchor as usize).min(source.len())].bytes().rev() {
        match byte {
            b'\n' => newlines += 1,
            byte if byte.is_ascii_whitespace() => {}
            _ => break,
        }
    }
    newlines >= 2
}

/// Whether the whitespace run immediately after `end` contains a blank line
/// (the gap between the last statement and the block closer).
fn has_trailing_edge_blank(source: &str, end: u32) -> bool {
    let mut newlines = 0;
    for byte in source[(end as usize).min(source.len())..].bytes() {
        match byte {
            b'\n' => newlines += 1,
            byte if byte.is_ascii_whitespace() => {}
            _ => break,
        }
    }
    newlines >= 2
}

impl Format for LastStatement {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            LastStatement::Return(ret) => {
                if ret.exprs.is_empty() {
                    crate::write!(f, [token("return")]);
                } else {
                    crate::write!(
                        f,
                        [group(format_with(|f| {
                            crate::write!(f, [token("return"), space()]);
                            crate::format_stmt::write_punctuated_exprs(f, &ret.exprs);
                        }))]
                    );
                }
            }
            LastStatement::Break(_) => crate::write!(f, [token("break")]),
            // Luau: `continue`.
            LastStatement::Continue(_) => crate::write!(f, [token("continue")]),
            // Parse-recovery placeholder: nothing to print.
            LastStatement::Error(_) => {}
        }
    }
}

/// Whether a block is simple enough to collapse onto one line: a single
/// statement with no terminator, or a lone terminator (return/break/continue).
pub(crate) fn is_simple_block(block: &Block) -> bool {
    let real_stmts = block
        .stmts
        .iter()
        .filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_)))
        .count();
    matches!((real_stmts, &block.last_stmt), (0, Some(_)) | (1, None))
}

/// Emit the line break between two adjacent items, upgraded to a blank line
/// when the source has one (or, on the source-less path, when a neighbor
/// wants surrounding blanks or the next statement's anchor requested one).
fn emit_separator(f: &mut Formatter, previous_end: u32, next_start: u32, synthetic_blank: bool) {
    let is_blank = match f.comments.source_text() {
        Some(source) => {
            // Bound the scan at an intervening comment so a blank line after
            // the comment doesn't get attributed to the statement gap.
            let gap_end = f
                .comments
                .peek_next_start()
                .unwrap_or(next_start)
                .min(next_start);
            has_blank_line(source, previous_end, gap_end)
        }
        None => synthetic_blank || f.comments.has_synthetic_blank_before(next_start),
    };
    if is_blank {
        crate::write!(f, [empty_line()]);
    } else {
        crate::write!(f, [hard_line()]);
    }
}

/// Emit the statement verbatim from source when it sits in an ignore /
/// `format off` region, otherwise format it normally. The source slice is
/// cloned first so the immutable borrow is released before writing.
fn emit_verbatim_or(
    f: &mut Formatter,
    start: u32,
    end: u32,
    is_ignored: bool,
    format: impl FnOnce(&mut Formatter),
) {
    let verbatim = if is_ignored || f.comments.is_format_disabled_at(start) {
        f.comments
            .source_text()
            .map(|source| source[start as usize..end as usize].to_string())
    } else {
        None
    };
    match verbatim {
        Some(slice) => {
            crate::write!(f, [text(slice)]);
            // The slice already contains the statement's inner comments;
            // without this they would re-emit as trailing own-line runs.
            f.comments.mark_printed_through(end);
        }
        None => format(f),
    }
}

/// Declarations that read better with a blank line around them when the input
/// carries no source to scan (synthetic ASTs).
fn stmt_wants_surrounding_blank(stmt: &Statement) -> bool {
    matches!(
        stmt,
        Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::GlobalFunction(_)
            | Statement::TypeDeclaration(_)
    )
}

/// Whether `source[start..end]` contains a blank line (two or more newlines).
fn has_blank_line(source: &str, start: u32, end: u32) -> bool {
    if start >= end {
        return false;
    }
    let end = end as usize;
    if end > source.len() {
        return false;
    }
    source[start as usize..end]
        .bytes()
        .filter(|&byte| byte == b'\n')
        .count()
        >= 2
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::comments::Comments;
    use crate::ir::{Format, Formatter};
    use crate::printer::{self, PrinterOptions};

    fn format(source: &str, version: LuaVersion) -> String {
        let parsed = luck_parser::parse(source, version);
        assert!(
            parsed.errors.is_empty(),
            "parse errors: {:?}",
            parsed.errors
        );
        let comments = Comments::from_source(&parsed.comments, source);
        let mut formatter = Formatter::with_context(crate::FormatOptions::default(), comments);
        formatter.emit_shebang();
        parsed.block.fmt(&mut formatter);
        formatter.emit_remaining_comments(true);
        let group_count = formatter.group_count();
        let elements = formatter.into_elements();
        let options = PrinterOptions {
            line_width: 100,
            use_tabs: true,
            indent_width: 4,
        };
        printer::print(&elements, group_count, &options)
    }

    #[test]
    fn statements_separated_by_newline() {
        let output = format("local a = 1\nlocal b = 2", LuaVersion::Lua54);
        let lines: Vec<&str> = output.trim().lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn blank_line_between_statements_preserved() {
        let output = format("local a = 1\n\n\nlocal b = 2", LuaVersion::Lua54);
        // The triple-newline collapses to exactly one blank line.
        assert!(output.contains("\n\n"));
        assert!(!output.contains("\n\n\n"));
    }

    #[test]
    fn single_newline_stays_single() {
        let output = format("local a = 1\nlocal b = 2", LuaVersion::Lua54);
        assert!(!output.trim().contains("\n\n"));
    }

    #[test]
    fn empty_statements_dropped() {
        let output = format("local a = 1;;;", LuaVersion::Lua54);
        assert_eq!(output.trim(), "local a = 1");
    }

    #[test]
    fn return_with_values() {
        let output = format("return 1, 2", LuaVersion::Lua54);
        assert!(output.contains("return"));
        assert!(output.contains('1'));
        assert!(output.contains('2'));
    }

    #[test]
    fn paren_guard_after_call() {
        // A `(`-starting statement must not glue onto the previous call.
        // The `;` in the input is what makes these parse as two statements;
        // without it the parser reads one chained call.
        let output = format("f();(g)()", LuaVersion::Lua54);
        assert!(output.contains(";("));
    }
}
