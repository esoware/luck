//! Block emission: the statement loop that runs the comment protocol and
//! keeps blank lines between statements.
//!
//! For each statement the loop flushes the comments stalled before it, emits
//! its leading comments (noting a `luck: ignore`), adds the `;` guard before a
//! leading `(`, and emits the statement and then its trailing comments. A
//! statement under `luck: ignore`, in a `format off` region, or outside the
//! format range goes out verbatim. One with an interior comment no emitter
//! claimed falls back to its own source, re-indented.

use core::iter;
use core::ops::Range;

use luck_ast::{Block, LastStatement, Statement, stmt_starts_with_paren};
use luck_token::{
    CompactString, LuaVersion, Span, is_lua_whitespace, line_breaks, offset_to_index,
    strip_line_break,
};

use crate::BlockNewlineGaps;
use crate::comments::{write_leading_comments, write_stalled_comments, write_trailing_comments};
use crate::ir::{Format, Formatter, empty_line, hard_line, token};

/// A statement the block loop prints.
#[derive(Debug, Clone, Copy)]
enum BlockItem<'block> {
    Stmt(&'block Statement),
    Last(&'block LastStatement),
}

impl BlockItem<'_> {
    fn span(self) -> Span {
        match self {
            Self::Stmt(stmt) => stmt.span(),
            Self::Last(last_stmt) => last_stmt.span(),
        }
    }

    /// Declarations that read better with a blank line around them when the
    /// input carries no source to scan (synthetic ASTs).
    ///
    /// A last statement (`return`, `break`, `continue`) never wants one of
    /// its own.
    const fn should_surround_with_blank(self) -> bool {
        match self {
            Self::Stmt(stmt) => matches!(
                stmt,
                Statement::FunctionDecl(_)
                    | Statement::LocalFunction(_)
                    | Statement::GlobalFunction(_)
                    | Statement::TypeDeclaration(_)
            ),
            Self::Last(_) => false,
        }
    }

    fn starts_with_paren(self) -> bool {
        match self {
            Self::Stmt(stmt) => stmt_starts_with_paren(stmt),
            Self::Last(_) => false,
        }
    }
}

impl Format for BlockItem<'_> {
    fn fmt(&self, formatter: &mut Formatter) {
        match self {
            Self::Stmt(stmt) => stmt.fmt(formatter),
            Self::Last(last_stmt) => last_stmt.fmt(formatter),
        }
    }
}

/// The items of `block` that produce output, in order. A bare `;` and a
/// parse-error placeholder print nothing, so they get no separator either.
fn printed_items(block: &Block) -> impl Iterator<Item = BlockItem<'_>> {
    let stmts = block
        .stmts
        .iter()
        .filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_) | Statement::Error(_)))
        .map(BlockItem::Stmt);
    let last_stmt = block
        .last_stmt
        .as_deref()
        .filter(|last_stmt| !matches!(last_stmt, LastStatement::Error(_)))
        .map(BlockItem::Last);
    stmts.chain(last_stmt)
}

/// Start offset of the first item that will produce output.
pub(crate) fn first_item_start(block: &Block) -> Option<u32> {
    first_item_span(block).map(|span| span.start)
}

/// Span of the first item that will produce output.
pub(crate) fn first_item_span(block: &Block) -> Option<Span> {
    printed_items(block).next().map(BlockItem::span)
}

/// What the separator before an item needs to know about the one before it.
#[derive(Debug, Clone, Copy)]
struct PreviousItem {
    end: u32,
    /// Whether it wants blank lines around it, which only counts on synthetic
    /// input, where there's no source to scan.
    should_surround_with_blank: bool,
    /// Whether it ended with a comment moved onto its own line, which rules
    /// out a blank after it.
    has_own_line_comment: bool,
}

impl Format for Block {
    fn fmt(&self, formatter: &mut Formatter) {
        let should_preserve_edge_blanks =
            formatter.options().block_newline_gaps == BlockNewlineGaps::Preserve;
        if should_preserve_edge_blanks && let Some(first_start) = first_item_start(self) {
            // A leading comment ends the gap at its own start.
            let anchor = formatter
                .comments()
                .peek_next_start()
                .filter(|start| *start < first_start)
                .unwrap_or(first_start);
            let has_blank = formatter
                .comments()
                .source_text()
                .zip(formatter.comments().source_version())
                .is_some_and(|(source, version)| has_leading_edge_blank(source, anchor, version));
            if has_blank {
                empty_line().fmt(formatter);
            }
        }

        let mut previous: Option<PreviousItem> = None;
        for item in printed_items(self) {
            let Span { start, end } = item.span();
            let should_surround_with_blank = item.should_surround_with_blank();

            if let Some(previous_item) = previous {
                write_separator(formatter, previous_item, start, should_surround_with_blank);
            }

            write_stalled_comments(formatter, start);
            let is_ignored = write_leading_comments(formatter, start);

            // A statement starting with `(` would reparse as a call on the
            // one before it (`f()` then `(g)()` reads as `f()(g)()`), so it
            // gets a `;` prefix, as in the compact printer.
            if previous.is_some() && item.starts_with_paren() {
                token(";").fmt(formatter);
            }

            // Statements outside the format range stay verbatim, so a
            // format-selection edit touches only the selection.
            let is_outside_range = formatter
                .format_range()
                .is_some_and(|range| end <= range.start || start >= range.end);

            write_verbatim_or(formatter, item, is_ignored || is_outside_range);
            let has_own_line_comment = write_trailing_comments(formatter, start, end);

            previous = Some(PreviousItem { end, should_surround_with_blank, has_own_line_comment });
        }

        if should_preserve_edge_blanks && let Some(last_item) = previous {
            let has_blank = formatter
                .comments()
                .source_text()
                .zip(formatter.comments().source_version())
                .is_some_and(|(source, version)| {
                    has_trailing_edge_blank(source, last_item.end, version)
                });
            if has_blank {
                empty_line().fmt(formatter);
            }
        }
    }
}

/// Whether the whitespace run immediately before `anchor` contains a blank
/// line (the gap between a block opener and its first statement).
fn has_leading_edge_blank(source: &str, anchor: u32, version: LuaVersion) -> bool {
    let preceding = source.get(..offset_to_index(anchor)).unwrap_or(source);
    let run_length = preceding.bytes().rev().take_while(|&byte| is_lua_whitespace(byte)).count();
    let run_start = preceding.len().saturating_sub(run_length);
    run_has_blank(preceding.get(run_start..).unwrap_or_default(), version)
}

/// Whether the whitespace run immediately after `end` contains a blank line
/// (the gap between the last statement and the block closer).
fn has_trailing_edge_blank(source: &str, end: u32, version: LuaVersion) -> bool {
    let following = source.get(offset_to_index(end)..).unwrap_or_default();
    let run_length = following.bytes().take_while(|&byte| is_lua_whitespace(byte)).count();
    run_has_blank(following.get(..run_length).unwrap_or_default(), version)
}

/// Whether `text` holds two or more line breaks.
fn run_has_blank(text: &str, version: LuaVersion) -> bool {
    line_breaks(text, version).nth(1).is_some()
}

/// Write the line break between two adjacent items.
///
/// It becomes a blank line when the source has one or, on the source-less
/// path, when a neighbor wants surrounding blanks or the next statement's
/// anchor asked for one. A comment the previous item moved onto its own line
/// rules the blank out, because it would sit between that comment and this
/// statement, where a reparse reads the comment as leading and drops the
/// blank.
fn write_separator(
    formatter: &mut Formatter,
    previous: PreviousItem,
    next_start: u32,
    should_surround_next_with_blank: bool,
) {
    let is_blank = match formatter.comments().source_text() {
        Some(_) if previous.has_own_line_comment => false,
        Some(source) => {
            // Stop at an intervening comment, so a blank line after it isn't
            // counted as part of this gap.
            let gap_end =
                formatter.comments().peek_next_start().unwrap_or(next_start).min(next_start);
            // A line break inside a printed trailing comment isn't a gap line.
            let gap_start = formatter
                .comments()
                .last_printed_end()
                .map_or(previous.end, |printed_end| printed_end.max(previous.end));
            formatter
                .comments()
                .source_version()
                .is_some_and(|version| has_blank_line(source, gap_start, gap_end, version))
        }
        None => {
            previous.should_surround_with_blank
                || should_surround_next_with_blank
                || formatter.comments().has_synthetic_blank_before(next_start)
        }
    };
    if is_blank {
        empty_line().fmt(formatter);
    } else {
        hard_line().fmt(formatter);
    }
}

/// Write the statement verbatim when it opted out of formatting or sits in a
/// `format off` region, and format it otherwise.
///
/// A formatted statement with an interior comment no emitter claimed is
/// replaced by its own source, re-indented, so the comment can't drift.
fn write_verbatim_or(formatter: &mut Formatter, item: BlockItem<'_>, is_opted_out: bool) {
    let Span { start, end } = item.span();
    let verbatim = if is_opted_out || formatter.comments().is_format_disabled_at(start) {
        formatter
            .comments()
            .source_text()
            .map(|source| CompactString::from(Span::new(start, end).text(source)))
    } else {
        None
    };
    if let Some(slice) = verbatim {
        // An opted-out statement keeps its layout down to the column, so the
        // slice goes out as one multi-line text.
        formatter.write_text(slice);
        // The slice already holds the statement's comments; left pending,
        // they'd print again after it.
        formatter.comments_mut().mark_printed_range(start, end);
        return;
    }

    let checkpoint = formatter.checkpoint();
    item.fmt(formatter);
    if formatter.comments().has_unhandled_in(start, end)
        && let Some(source) = formatter.comments().source_text()
        && let Some(version) = formatter.comments().source_version()
    {
        // An unclaimed interior comment keeps the statement as written, so
        // the comment can't drift across tokens. The statement didn't opt out
        // of formatting, though, so it still takes the block's indentation
        // and its runs of blank lines still collapse to one.
        let lines = reindented_lines(source, start, end, version);
        formatter.restore(checkpoint);
        for (index, line) in lines.into_iter().enumerate() {
            if index > 0 {
                hard_line().fmt(formatter);
            }
            if !line.is_empty() {
                formatter.write_text(line);
            }
        }
        formatter.comments_mut().mark_printed_range(start, end);
    }
}

/// `source[start..end]` split into lines, with the statement's own
/// indentation stripped from its continuation lines so the printer's indent
/// replaces the original column.
///
/// Lines split only at newlines between tokens. A newline inside a long
/// string or long comment is part of that token, so it stays in its line and
/// goes out as written; re-indenting around it would change the literal.
fn reindented_lines(source: &str, start: u32, end: u32, version: LuaVersion) -> Vec<CompactString> {
    let text = Span::new(start, end).text(source);
    let preceding = Span::new(0, start).text(source);
    let line_start = line_breaks(preceding, version).last().map_or(0, |line_break| line_break.end);
    let line_prefix = preceding.get(line_start..).unwrap_or_default();
    // A statement that shares its line with earlier code has no indentation
    // of its own to strip.
    let base_indent = if line_prefix.bytes().all(|byte| byte == b' ' || byte == b'\t') {
        line_prefix
    } else {
        ""
    };

    let mut lines = Vec::new();
    let mut cursor = 0;
    let separators = separable_newlines(text, version);
    for offset in separators.into_iter().chain(iter::once(text.len())) {
        let line = text.get(cursor..offset).unwrap_or_default();
        lines.push(CompactString::from(if lines.is_empty() {
            line
        } else {
            line.strip_prefix(base_indent).unwrap_or(line)
        }));
        let rest = text.as_bytes().get(offset..).unwrap_or_default();
        cursor = text.len().saturating_sub(strip_line_break(rest, version).unwrap_or(rest).len());
    }
    lines
}

/// The start offsets of the line breaks in `text` that separate lines, as
/// opposed to ones inside a token.
///
/// A long string or long comment carries its line breaks as content, and a
/// `\` continues a short string across one. Lexing tells them apart: a line
/// break outside every token and comment span sits between two tokens.
fn separable_newlines(text: &str, version: LuaVersion) -> Vec<usize> {
    let lexed = luck_lexer::lex(text, version);
    // A parsed statement always lexes. If this text doesn't, leave it unsplit,
    // since whatever the lexer couldn't close may be a literal that
    // re-indenting would change.
    if !lexed.errors.is_empty() {
        return Vec::new();
    }
    let mut spans: Vec<Range<usize>> = lexed
        .tokens
        .iter()
        .map(|token| token.span)
        .chain(lexed.comments.iter().map(|comment| comment.span))
        .filter(|span| span.end > span.start)
        .map(Range::from)
        .collect();
    spans.sort_unstable_by_key(|span| span.start);

    let mut upcoming_spans = spans.into_iter().peekable();
    let mut separators = Vec::new();
    for line_break in line_breaks(text, version) {
        while upcoming_spans.next_if(|span| span.end <= line_break.start).is_some() {}
        let is_inside = upcoming_spans.peek().is_some_and(|span| span.start <= line_break.start);
        if !is_inside {
            separators.push(line_break.start);
        }
    }
    separators
}

/// Whether `source[start..end]` contains a blank line (two or more line
/// breaks).
pub(crate) fn has_blank_line(source: &str, start: u32, end: u32, version: LuaVersion) -> bool {
    run_has_blank(Span::new(start, end).text(source), version)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    #[test]
    fn newlines_inside_a_token_do_not_separate_lines() {
        let separable = |text| super::separable_newlines(text, LuaVersion::Lua54);
        assert_eq!(separable("a\nb\nc"), vec![1, 3]);
        // Long string, long comment, and level-2 bracket.
        assert_eq!(separable("a\n[[b\nc]]\nd"), vec![1, 9]);
        assert_eq!(separable("a\n--[[b\nc]]\nd"), vec![1, 11]);
        assert_eq!(separable("a\n[==[b\nc]==]\nd"), vec![1, 13]);
        // A level mismatch doesn't close the bracket.
        assert_eq!(separable("[==[a\nb]=]\nc"), Vec::<usize>::new());
        // `]]` without an opener is just two tokens.
        assert_eq!(separable("a]]\nb"), vec![3]);
        // A short string continued across the break, and `\z`.
        assert_eq!(separable("\"a\\\nb\"\nc"), vec![6]);
        assert_eq!(separable("\"a\\z\n  b\"\nc"), vec![9]);
        // A trailing `\` in a comment escapes nothing, and `[[` inside a
        // short string opens nothing.
        assert_eq!(separable("a -- see C:\\\nb"), vec![12]);
        assert_eq!(separable("\"[[\"\nb"), vec![4]);
    }
}
