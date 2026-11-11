//! Block emission: the statement loop that runs the comment protocol and
//! keeps blank lines between statements.
//!
//! For each statement the loop flushes the comments stalled before it, emits
//! its leading comments (noting a `luck: ignore`), adds the `;` guard before a
//! leading `(`, and emits the statement and then its trailing comments. A
//! `format off` region goes out byte for byte as one verbatim item, and a
//! statement under `luck: ignore` keeps its layout, re-indented to its block.
//! A statement with an interior comment no emitter claimed falls back to its
//! own source, re-indented.

use core::iter::{self, Peekable};
use core::ops::Range;

use luck_ast::{Block, LastStatement, Statement, Visitor, stmt_starts_with_paren};
use luck_token::{
    CompactString, LuaVersion, Span, is_lua_whitespace, line_breaks, offset_to_index,
    strip_line_break,
};

use crate::comments::{
    is_code_free_byte, own_line_start, write_dangling_comments, write_leading_comments,
    write_stalled_comments, write_tail_comments, write_trailing_comments,
};
use crate::ir::{Format, Formatter, empty_line, hard_line, indent, token};
use crate::{BlockNewlineGaps, Comments, FormatOptions};

#[derive(Debug, Clone, Copy)]
enum BlockItem<'block> {
    Stmt(&'block Statement),
    Last(&'block LastStatement),
    Verbatim { span: Span, has_leading_paren: bool },
}

impl BlockItem<'_> {
    fn span(self) -> Span {
        match self {
            Self::Stmt(stmt) => stmt.span(),
            Self::Last(last_stmt) => last_stmt.span(),
            Self::Verbatim { span, .. } => span,
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
            Self::Last(_) | Self::Verbatim { .. } => false,
        }
    }

    fn starts_with_paren(self) -> bool {
        match self {
            Self::Stmt(stmt) => stmt_starts_with_paren(stmt),
            Self::Last(_) => false,
            Self::Verbatim { has_leading_paren, .. } => has_leading_paren,
        }
    }
}

impl Format for BlockItem<'_> {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Stmt(stmt) => stmt.fmt(f),
            Self::Last(last_stmt) => last_stmt.fmt(f),
            Self::Verbatim { span, .. } => write_exact(f, *span),
        }
    }
}

/// A bare `;` and a parse-error placeholder print nothing, so they get no separator either.
fn printed_items(block: &Block) -> impl Iterator<Item = BlockItem<'_>> + Clone {
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

pub(crate) fn first_item_start(block: &Block) -> Option<u32> {
    first_item_span(block).map(|span| span.start)
}

pub(crate) fn first_item_span(block: &Block) -> Option<Span> {
    printed_items(block).next().map(BlockItem::span)
}

#[derive(Debug, Clone, Copy)]
struct PreviousItem {
    end: u32,
    /// Whether it wants blank lines around it, which only counts on synthetic
    /// input, where there's no source to scan.
    should_surround_with_blank: bool,
}

/// An empty block with nothing to print writes nothing, so no blank line comes before its
/// closer.
pub(crate) struct IndentedBlock<'block>(pub &'block Block);

impl Format for IndentedBlock<'_> {
    fn fmt(&self, f: &mut Formatter) {
        if first_item_span(self.0).is_some()
            || f.comments().has_dangling_comments(self.0.span.start)
        {
            indent((hard_line(), self.0)).fmt(f);
        }
    }
}

impl Format for Block {
    fn fmt(&self, f: &mut Formatter) {
        let should_preserve_edge_blanks =
            f.options().block_newline_gaps == BlockNewlineGaps::Preserve;
        if should_preserve_edge_blanks && let Some(first_start) = first_item_start(self) {
            // A leading comment ends the gap at its own start.
            let anchor = f
                .comments()
                .peek_next_start()
                .filter(|start| *start < first_start)
                .unwrap_or(first_start);
            if has_blank_line_above(f, anchor) {
                empty_line().fmt(f);
            }
        }

        let Some(last_end) = write_items(f, printed_items(self)) else {
            write_dangling_comments(f, self.span.start);
            return;
        };
        write_tail_comments(f, last_end);

        if should_preserve_edge_blanks {
            let printed_end =
                f.comments().last_printed_end().map_or(last_end, |end| end.max(last_end));
            let has_blank =
                f.comments().source_text().zip(f.comments().source_version()).is_some_and(
                    |(source, version)| has_trailing_edge_blank(source, printed_end, version),
                );
            if has_blank {
                empty_line().fmt(f);
            }
        }
    }
}

fn write_items<'block, Items>(f: &mut Formatter, items: Items) -> Option<u32>
where
    Items: Iterator<Item = BlockItem<'block>> + Clone,
{
    let mut remaining = items.peekable();
    let mut previous: Option<PreviousItem> = None;
    while let Some(item) =
        next_item(f, &mut remaining, previous.map_or(0, |last_item| last_item.end))
    {
        let Span { start, end } = item.span();
        let should_surround_with_blank = item.should_surround_with_blank();

        if let Some(previous_item) = previous {
            write_separator(f, previous_item, start, should_surround_with_blank);
        }

        let has_stalled_ignore = write_stalled_comments(f, start);
        let has_leading_ignore = write_leading_comments(f, start);

        // A statement starting with `(` would reparse as a call on the
        // one before it (`f()` then `(g)()` reads as `f()(g)()`), so it
        // gets a `;` prefix, as in the compact printer. A `format off`
        // region can't gain one, so its guard goes on a line above it.
        if previous.is_some() && item.starts_with_paren() {
            token(";").fmt(f);
            if matches!(item, BlockItem::Verbatim { .. }) {
                hard_line().fmt(f);
            }
        }

        write_verbatim_or(f, item, has_stalled_ignore || has_leading_ignore);
        write_trailing_comments(f, start, end);

        previous = Some(PreviousItem { end, should_surround_with_blank });
    }
    previous.map(|last_item| last_item.end)
}

/// A region wholly inside the statement is left to it: a nested block claims
/// it, and otherwise the statement's own verbatim fallback does.
///
/// A region that crosses this block's opener or closer can't be claimed here either, so its `off`
/// comment stays pending and the enclosing statement goes verbatim.
fn next_item<'block, Items>(
    f: &Formatter,
    items: &mut Peekable<Items>,
    position: u32,
) -> Option<BlockItem<'block>>
where
    Items: Iterator<Item = BlockItem<'block>> + Clone,
{
    let comments = f.comments();
    let Some(region) = comments.next_disabled_region(position) else {
        return items.next();
    };
    if let Some(span) = items.peek().map(|item| item.span()) {
        let is_past = region.start >= span.end;
        // A region inside a statement starts past the statement's first token.
        let is_inside = span.start < region.start && region.end <= span.end;
        if is_past || is_inside {
            return items.next();
        }
    }

    let source = comments.source_text()?;
    let mut run = region;
    let mut lookahead = items.clone();
    let mut absorbed: Option<Range<u32>> = None;
    let mut has_leading_paren = false;
    loop {
        while let Some(item) = lookahead.next_if(|item| item.span().start < run.end) {
            let span = item.span();
            if absorbed.is_none() {
                has_leading_paren = item.starts_with_paren();
            }
            run.start = run.start.min(own_line_start(source, span.start).unwrap_or(span.start));
            run.end = run.end.max(span.end);
            absorbed = Some(absorbed.map_or(span.start, |range| range.start)..span.end);
        }
        match comments.next_disabled_region(run.end) {
            Some(next_region) if next_region.start < run.end => run.end = next_region.end,
            Some(_) | None => break,
        }
    }

    let stays_in_block = absorbed.map_or_else(
        || comments.is_free_of_tokens(run.start, run.end),
        |range| {
            comments.is_free_of_tokens(run.start, range.start)
                && comments.is_free_of_tokens(range.end, run.end)
        },
    );
    if !stays_in_block {
        return items.next();
    }
    *items = lookahead;
    Some(BlockItem::Verbatim { span: Span::new(run.start, run.end), has_leading_paren })
}

struct InnermostBlock<'ast, 'range> {
    range: &'range Range<u32>,
    found: &'ast Block,
}

impl<'ast> Visitor<'ast> for InnermostBlock<'ast, '_> {
    fn visit_block(&mut self, block: &'ast Block) {
        let first_start = first_item_start(block);
        let last_end = printed_items(block).last().map(|item| item.span().end);
        if first_start.is_some_and(|start| start <= self.range.start)
            && last_end.is_some_and(|end| self.range.end <= end)
        {
            self.found = block;
            self.walk_block(block);
        }
    }
}

/// Format the statements of `block`, or of its innermost nested block that
/// holds the whole of `range`, that overlap `range`.
///
/// Returns the source span they replace, from the start of the first one's
/// line to the end of the last one's trailing comment, and the text to put
/// there. `None` when no statement overlaps `range`.
pub(crate) fn format_selection(
    block: &Block,
    mut comments: Comments,
    options: &FormatOptions,
    range: &Range<u32>,
) -> Option<(Span, String)> {
    let mut finder = InnermostBlock { range, found: block };
    finder.visit_block(block);

    let is_selected = |item: &BlockItem<'_>| {
        let Span { start, end } = item.span();
        end > range.start && start < range.end
    };
    let items = printed_items(finder.found)
        .skip_while(|item| !is_selected(item))
        .take_while(|item| is_selected(item));
    let first_start = items.clone().next()?.span().start;
    let last_end = items.clone().last()?.span().end;

    let source = comments.source_text()?;
    let line_start = own_line_start(source, first_start).unwrap_or(first_start);
    let base_indent = Span::new(line_start, first_start).text(source).to_owned();
    comments.restrict_to(Span::new(line_start, last_end));

    let mut f = Formatter::new(*options, comments);
    write_items(&mut f, items);
    let end = f.comments().last_printed_end().map_or(last_end, |end| end.max(last_end));
    Some((Span::new(line_start, end), f.print(&base_indent)))
}

/// Whether the source has a blank line right above `offset`. Synthetic input
/// has no source and never does.
pub(crate) fn has_blank_line_above(f: &Formatter, offset: u32) -> bool {
    let comments = f.comments();
    comments
        .source_text()
        .zip(comments.source_version())
        .is_some_and(|(source, version)| has_leading_edge_blank(source, offset, version))
}

/// Whether the whitespace run immediately before `anchor` contains a blank
/// line (the gap between a block opener and its first statement).
pub(crate) fn has_leading_edge_blank(source: &str, anchor: u32, version: LuaVersion) -> bool {
    let preceding = source.get(..offset_to_index(anchor)).unwrap_or(source);
    let run_length = preceding.bytes().rev().take_while(|&b| is_lua_whitespace(b)).count();
    let run_start = preceding.len().saturating_sub(run_length);
    run_has_blank(preceding.get(run_start..).unwrap_or_default(), version)
}

/// Whether the whitespace and `;` run immediately after `end` contains a blank
/// line (the gap between the last statement and the block closer).
fn has_trailing_edge_blank(source: &str, end: u32, version: LuaVersion) -> bool {
    let following = source.get(offset_to_index(end)..).unwrap_or_default();
    let run_length = following.bytes().take_while(|&b| is_code_free_byte(b)).count();
    run_has_blank(following.get(..run_length).unwrap_or_default(), version)
}

/// Whether `text` holds a blank line: two line breaks with no `;` between them,
/// since a line holding only a `;` isn't blank.
fn run_has_blank(text: &str, version: LuaVersion) -> bool {
    text.split(';').any(|segment| line_breaks(segment, version).nth(1).is_some())
}

/// Write the line break between two adjacent items.
///
/// It becomes a blank line when the source has one or, on the source-less
/// path, when a neighbor wants surrounding blanks or the next statement's
/// anchor asked for one.
fn write_separator(
    f: &mut Formatter,
    previous: PreviousItem,
    next_start: u32,
    should_surround_next_with_blank: bool,
) {
    let is_blank = f.comments().source_text().map_or_else(
        || {
            previous.should_surround_with_blank
                || should_surround_next_with_blank
                || f.comments().has_synthetic_blank_before(next_start)
        },
        |source| {
            // Stop at an intervening comment, so a blank line after it isn't
            // counted as part of this gap.
            let gap_end = f.comments().peek_next_start().unwrap_or(next_start).min(next_start);
            // A line break inside a printed trailing comment isn't a gap line.
            let gap_start = f
                .comments()
                .last_printed_end()
                .map_or(previous.end, |printed_end| printed_end.max(previous.end));
            f.comments()
                .source_version()
                .is_some_and(|version| has_blank_line(source, gap_start, gap_end, version))
        },
    );
    if is_blank {
        empty_line().fmt(f);
    } else {
        hard_line().fmt(f);
    }
}

/// A statement that opted out or has such a comment takes its block's
/// indentation, keeping its line breaks only if it opted out. One holding a
/// `format off` region goes out byte for byte.
fn write_verbatim_or(f: &mut Formatter, item: BlockItem<'_>, is_opted_out: bool) {
    if matches!(item, BlockItem::Verbatim { .. }) {
        item.fmt(f);
        return;
    }
    let Span { start, end } = item.span();
    if is_opted_out && write_reindented(f, start, end, true) {
        return;
    }

    let checkpoint = f.checkpoint();
    item.fmt(f);
    if f.comments().has_unhandled_in(start, end) {
        let is_in_region = f.comments().overlaps_disabled_region(Span::new(start, end));
        f.restore(checkpoint);
        if is_in_region {
            let line_start = f
                .comments()
                .source_text()
                .and_then(|source| own_line_start(source, start))
                .unwrap_or(start);
            write_exact(f, Span::new(line_start, end));
        } else {
            write_reindented(f, start, end, false);
        }
    }
}

fn write_exact(f: &mut Formatter, span: Span) {
    if let Some(text) = f.comments().source_text().map(|source| span.text(source).to_owned()) {
        f.write_verbatim(text);
        f.comments_mut().mark_printed_range(span.start, span.end);
    }
}

/// Write `source[start..end]` line by line with the printer's indentation, and
/// count its comments as printed. Line breaks are the source's when
/// `should_keep_breaks`, and the printer's otherwise.
///
/// Returns false when there's no source to copy.
fn write_reindented(f: &mut Formatter, start: u32, end: u32, should_keep_breaks: bool) -> bool {
    let (Some(source), Some(version)) = (f.comments().source_text(), f.comments().source_version())
    else {
        return false;
    };
    for (line, line_break) in reindented_lines(source, start, end, version) {
        if !line.is_empty() {
            f.write_text(line);
        }
        if line_break.is_empty() {
            continue;
        }
        if should_keep_breaks {
            f.write_verbatim(line_break);
        } else {
            hard_line().fmt(f);
        }
    }
    f.comments_mut().mark_printed_range(start, end);
    true
}

/// The statement's own indentation comes off its continuation lines, so the
/// printer's indent replaces the original column.
///
/// Lines split only at newlines between tokens: a newline inside a long string or long comment is
/// part of that token and goes out as written, since re-indenting would change it.
fn reindented_lines(
    source: &str,
    start: u32,
    end: u32,
    version: LuaVersion,
) -> Vec<(CompactString, CompactString)> {
    let text = Span::new(start, end).text(source);
    let base_indent = own_line_start(source, start)
        .map_or("", |line_start| Span::new(line_start, start).text(source));

    let mut lines = Vec::new();
    let mut cursor = 0;
    let separators = separable_newlines(text, version);
    for offset in separators.into_iter().chain(iter::once(text.len())) {
        let line = text.get(cursor..offset).unwrap_or_default();
        let rest = text.as_bytes().get(offset..).unwrap_or_default();
        let next_cursor =
            text.len().saturating_sub(strip_line_break(rest, version).unwrap_or(rest).len());
        lines.push((
            CompactString::from(if lines.is_empty() {
                line
            } else {
                line.strip_prefix(base_indent).unwrap_or(line)
            }),
            CompactString::from(text.get(offset..next_cursor).unwrap_or_default()),
        ));
        cursor = next_cursor;
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
        .chain(lexed.comments.iter().map(|c| c.span))
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
