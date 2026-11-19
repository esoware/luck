//! Block emission: the statement loop that places the comments between
//! statements and keeps blank lines between them.
//!
//! For each statement the loop writes the comments before it (noting a
//! `luck: ignore`), adds the `;` guard before a leading `(`, and writes the
//! statement and then the comments after it on its line. A `format off`
//! region goes out byte for byte as one verbatim item, and a statement under
//! `luck: ignore` keeps its layout, re-indented to its block.

use core::iter::{self, Peekable};
use core::ops::Range;

use luck_ast::{Block, Chunk, GetSpan as _, Statement, Visit, stmt_starts_with_paren};
use luck_token::{
    LuaVersion, Span, is_lua_whitespace, line_breaks, offset_to_index, strip_line_break,
};

use crate::comments::{is_code_free_byte, own_line_start};
use crate::ir::Formatter;
use crate::sort_requires::{RequireRun, sortable_require};
use crate::{BlockNewlineGaps, Comments, FormatOptions};

#[derive(Debug, Clone, Copy)]
enum BlockItem<'block, 'ast> {
    Stmt(&'block Statement<'ast>),
    Verbatim { span: Span, has_leading_paren: bool },
}

impl BlockItem<'_, '_> {
    /// A `return`'s span takes the `;` after it, which the formatter drops,
    /// so it ends at its last value here: a comment there trails the values.
    fn span(self) -> Span {
        match self {
            Self::Stmt(Statement::Return(return_stmt)) => {
                let span = return_stmt.span;
                return_stmt.exprs.last().map_or(span, |last| Span::new(span.start, last.span().end))
            }
            Self::Stmt(stmt) => stmt.span(),
            Self::Verbatim { span, .. } => span,
        }
    }

    /// Declarations that read better with a blank line around them when the
    /// input carries no source to scan (synthetic ASTs).
    const fn should_surround_with_blank(self) -> bool {
        match self {
            Self::Stmt(stmt) => matches!(
                stmt,
                Statement::FunctionDecl(_)
                    | Statement::LocalFunction(_)
                    | Statement::GlobalFunction(_)
                    | Statement::TypeDeclaration(_)
            ),
            Self::Verbatim { .. } => false,
        }
    }

    fn starts_with_paren(self) -> bool {
        match self {
            Self::Stmt(stmt) => stmt_starts_with_paren(stmt),
            Self::Verbatim { has_leading_paren, .. } => has_leading_paren,
        }
    }
}

/// A parse-error placeholder prints nothing, so it gets no separator either.
fn printed_items<'block, 'ast>(
    block: &'block Block<'ast>,
) -> impl Iterator<Item = BlockItem<'block, 'ast>> + Clone {
    block.stmts.iter().filter(|stmt| !matches!(stmt, Statement::Error(_))).map(BlockItem::Stmt)
}

pub(crate) fn first_item_start(block: &Block<'_>) -> Option<u32> {
    first_item_span(block).map(|span| span.start)
}

pub(crate) fn first_item_span(block: &Block<'_>) -> Option<Span> {
    printed_items(block).next().map(BlockItem::span)
}

#[derive(Debug, Clone, Copy)]
struct PreviousItem {
    end: u32,
    /// Whether it wants blank lines around it, which only counts on synthetic
    /// input, where there's no source to scan.
    should_surround_with_blank: bool,
}

impl<'ast> Formatter<'ast> {
    /// An empty block with nothing to print writes nothing, so no blank line
    /// comes before its closer.
    pub(crate) fn write_indented_block(&mut self, block: &Block<'ast>) {
        if self.has_printable_content(block) {
            self.indent(|f| {
                f.hard_line();
                f.write_block(block);
            });
        }
    }

    pub(crate) fn has_printable_content(&self, block: &Block<'_>) -> bool {
        first_item_span(block).is_some() || self.comments().has_dangling_comments(block.span.start)
    }

    pub(crate) fn write_block(&mut self, block: &Block<'ast>) {
        let should_preserve_edge_blanks =
            self.options().block_newline_gaps == BlockNewlineGaps::Preserve;
        if should_preserve_edge_blanks
            && first_item_start(block).is_some_and(|start| self.has_blank_line_before(start))
        {
            self.empty_line();
        }

        let should_sort_requires = self.sorts_requires();
        let Some(last_end) = self.write_items(printed_items(block), should_sort_requires) else {
            self.write_dangling_comments(block.span.start);
            return;
        };
        self.write_tail_comments(last_end);

        if should_preserve_edge_blanks {
            let printed_end =
                self.comments().last_printed_end().map_or(last_end, |end| end.max(last_end));
            let has_blank = self.comments().source().is_some_and(|(source, version)| {
                has_trailing_edge_blank(source, printed_end, version)
            });
            if has_blank {
                self.empty_line();
            }
        }
    }

    fn write_items<'block, Items>(
        &mut self,
        items: Items,
        should_sort_requires: bool,
    ) -> Option<u32>
    where
        Items: Iterator<Item = BlockItem<'block, 'ast>> + Clone,
        'ast: 'block,
    {
        let mut remaining = items.peekable();
        let mut previous: Option<PreviousItem> = None;
        let mut requires = RequireRun::default();
        while let Some(item) =
            self.next_item(&mut remaining, previous.map_or(0, |last_item| last_item.end))
        {
            let Span { start, end } = item.span();
            let should_surround_with_blank = item.should_surround_with_blank();

            let is_blank_before = previous.is_some_and(|previous_item| {
                self.write_separator(previous_item, start, should_surround_with_blank)
            });

            let before_comments = self.element_count();
            let is_opted_out = self.write_stmt_leading_comments(start);
            let has_comment_lines = self.element_count() != before_comments;

            // A statement starting with `(` would reparse as a call on the
            // one before it (`f()` then `(g)()` reads as `f()(g)()`), so it
            // gets a `;` prefix, as in the compact printer. A `format off`
            // region can't gain one, so its guard goes on a line above it.
            if previous.is_some() && item.starts_with_paren() {
                self.token(";");
                if matches!(item, BlockItem::Verbatim { .. }) {
                    self.hard_line();
                }
            }

            let item_start = self.element_count();
            self.write_verbatim_or(item, Span { start, end }, is_opted_out);
            let has_multi_line_trailer = self.write_stmt_trailing_comments(start, end);

            if should_sort_requires {
                // Requires move as whole lines, so a run ends at a blank line or a
                // comment line between two of them.
                if is_blank_before || has_comment_lines {
                    requires.finish(self);
                }
                let sortable = match item {
                    BlockItem::Stmt(stmt)
                        if !is_opted_out
                            && !has_multi_line_trailer
                            && !self.comments().overlaps_disabled_region(Span::new(start, end)) =>
                    {
                        sortable_require(stmt)
                    }
                    BlockItem::Stmt(_) | BlockItem::Verbatim { .. } => None,
                };
                match sortable {
                    Some(require) => requires.push(require, item_start..self.element_count()),
                    None => requires.finish(self),
                }
            }

            previous = Some(PreviousItem { end, should_surround_with_blank });
        }
        requires.finish(self);
        previous.map(|last_item| last_item.end)
    }

    /// A region wholly inside the statement is left to it: a nested block
    /// claims it, and otherwise the statement goes out as written.
    ///
    /// A region that crosses this block's opener or closer can't be claimed
    /// here either, so its `off` comment stays pending and the enclosing
    /// statement goes out as written.
    fn next_item<'block, Items>(
        &self,
        items: &mut Peekable<Items>,
        position: u32,
    ) -> Option<BlockItem<'block, 'ast>>
    where
        Items: Iterator<Item = BlockItem<'block, 'ast>> + Clone,
    {
        let comments = self.comments();
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

    /// Whether the source has a blank line right above the node at `start`,
    /// or above the first comment leading it. Synthetic input has no source
    /// and never does.
    pub(crate) fn has_blank_line_before(&self, start: u32) -> bool {
        let comments = self.comments();
        let anchor = comments.peek_next_start().filter(|comment| *comment < start).unwrap_or(start);
        comments
            .source()
            .is_some_and(|(source, version)| has_leading_edge_blank(source, anchor, version))
    }

    /// Write the line break between two adjacent items, and return whether
    /// it's a blank line.
    ///
    /// It becomes one when the source has one or, on the source-less path,
    /// when a neighbor wants surrounding blanks or the next statement's anchor
    /// asked for one.
    fn write_separator(
        &mut self,
        previous: PreviousItem,
        next_start: u32,
        should_surround_next_with_blank: bool,
    ) -> bool {
        let comments = self.comments();
        let is_blank = comments.source().map_or_else(
            || {
                previous.should_surround_with_blank
                    || should_surround_next_with_blank
                    || comments.has_synthetic_blank_before(next_start)
            },
            |(source, version)| {
                // Stop at an intervening comment, so a blank line after it isn't
                // counted as part of this gap.
                let gap_end = comments.peek_next_start().unwrap_or(next_start).min(next_start);
                // A line break inside a printed trailing comment isn't a gap line.
                let gap_start = comments
                    .last_printed_end()
                    .map_or(previous.end, |printed_end| printed_end.max(previous.end));
                has_blank_line(source, gap_start, gap_end, version)
            },
        );
        self.line_break(is_blank);
        is_blank
    }

    fn write_item(&mut self, item: BlockItem<'_, 'ast>) {
        let end = item.span().end;
        match item {
            BlockItem::Stmt(stmt) => self.inside(end, |f| f.write_stmt(stmt)),
            BlockItem::Verbatim { span, .. } => self.write_exact(span),
        }
    }

    /// A statement that opted out keeps its layout, with its block's
    /// indentation.
    ///
    /// One that holds a `format off` region no block inside it claims goes
    /// out byte for byte, which only writing it can tell: the region's `off`
    /// comment is left unprinted.
    fn write_verbatim_or(&mut self, item: BlockItem<'_, 'ast>, span: Span, is_opted_out: bool) {
        let Span { start, end } = span;
        let is_region = matches!(item, BlockItem::Verbatim { .. });
        if !is_region && is_opted_out && self.write_reindented(start, end) {
            return;
        }
        if is_region || !self.comments().overlaps_disabled_region(span) {
            self.write_item(item);
            return;
        }
        let checkpoint = self.checkpoint();
        self.write_item(item);
        if self.comments().has_comment_in(start, end) {
            self.restore(checkpoint);
            let line_start = self
                .comments()
                .source_text()
                .and_then(|source| own_line_start(source, start))
                .unwrap_or(start);
            self.write_exact(Span::new(line_start, end));
        }
    }

    fn write_exact(&mut self, span: Span) {
        if let Some(source) = self.comments().source_text() {
            self.verbatim(span.text(source));
            self.comments_mut().mark_printed_range(span.start, span.end);
        }
    }

    /// Write `source[start..end]` line by line with the printer's indentation
    /// and the source's line breaks, and count its comments as printed.
    ///
    /// Returns false when there's no source to copy.
    fn write_reindented(&mut self, start: u32, end: u32) -> bool {
        let Some((source, version)) = self.comments().source() else {
            return false;
        };
        for (line, line_break) in reindented_lines(source, start, end, version) {
            if !line.is_empty() {
                self.text(line);
            }
            if !line_break.is_empty() {
                self.verbatim(line_break);
            }
        }
        self.comments_mut().mark_printed_range(start, end);
        true
    }
}

struct InnermostBlock<'tree, 'range> {
    range: &'range Range<u32>,
    found: &'tree Block<'tree>,
}

impl<'tree> Visit<'tree> for InnermostBlock<'tree, '_> {
    fn visit_block(&mut self, block: &'tree Block<'tree>) {
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

/// Format the statements of `chunk`, or of its innermost nested block that
/// holds the whole of `range`, that overlap `range`.
///
/// Returns the source span they replace, from the start of the first one's
/// line to the end of the last one's trailing comment, and the text to put
/// there. `None` when no statement overlaps `range`.
pub(crate) fn format_selection<'ast>(
    allocator: &'ast luck_allocator::Allocator,
    chunk: &'ast Chunk<'ast>,
    mut comments: Comments<'ast>,
    options: &FormatOptions,
    range: &Range<u32>,
) -> Option<(Span, String)> {
    let mut finder = InnermostBlock { range, found: &chunk.block };
    finder.visit_block(&chunk.block);

    let is_selected = |item: &BlockItem<'_, '_>| {
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
    let base_indent = Span::new(line_start, first_start).text(source);
    comments.restrict_to(Span::new(line_start, last_end));

    let mut f = Formatter::new(allocator, chunk.version, *options, comments);
    f.write_items(items, false);
    let end = f.comments().last_printed_end().map_or(last_end, |end| end.max(last_end));
    Some((Span::new(line_start, end), f.print(base_indent)))
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
///
/// The text is a gap between tokens, a few bytes as a rule, so one walk beats
/// searching it.
const fn run_has_blank(text: &str, version: LuaVersion) -> bool {
    let mut remaining = text.as_bytes();
    let mut has_break = false;
    while let Some((&b, rest)) = remaining.split_first() {
        if let Some(after_break) = strip_line_break(remaining, version) {
            if has_break {
                return true;
            }
            has_break = true;
            remaining = after_break;
        } else {
            has_break &= b != b';';
            remaining = rest;
        }
    }
    false
}

pub(crate) fn has_blank_line(source: &str, start: u32, end: u32, version: LuaVersion) -> bool {
    run_has_blank(Span::new(start, end).text(source), version)
}

/// The statement's own indentation comes off its continuation lines, so the
/// printer's indent replaces the original column. Each line comes with the
/// line break after it, empty for the last.
///
/// Lines split only at newlines between tokens: a newline inside a long string or long comment is
/// part of that token and goes out as written, since re-indenting would change it.
fn reindented_lines(source: &str, start: u32, end: u32, version: LuaVersion) -> Vec<(&str, &str)> {
    let text = Span::new(start, end).text(source);
    let base_indent = own_line_start(source, start)
        .map_or("", |line_start| Span::new(line_start, start).text(source));

    let mut lines = Vec::new();
    let mut cursor = 0;
    let separators = separable_newlines(text, version);
    for offset in separators.into_iter().chain(iter::once(text.len())) {
        let written = text.get(cursor..offset).unwrap_or_default();
        let rest = text.as_bytes().get(offset..).unwrap_or_default();
        let next_cursor =
            text.len().saturating_sub(strip_line_break(rest, version).unwrap_or(rest).len());
        let line = if lines.is_empty() {
            written
        } else {
            written.strip_prefix(base_indent).unwrap_or(written)
        };
        lines.push((line, text.get(offset..next_cursor).unwrap_or_default()));
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

#[cfg(test)]
mod tests {
    use luck_token::{LuaVersion, line_breaks};

    #[test]
    fn a_blank_line_is_two_breaks_with_no_semicolon_between() {
        // Every gap of up to six bytes from these, against the definition.
        let pieces = ['\n', '\r', ';', ' ', 'x'];
        let mut gaps = vec![String::new()];
        let mut longest = gaps.clone();
        for _ in 0..6u8 {
            longest = longest
                .iter()
                .flat_map(|gap| pieces.iter().map(move |piece| format!("{gap}{piece}")))
                .collect();
            gaps.extend(longest.iter().cloned());
        }
        for version in [LuaVersion::Lua54, LuaVersion::Luau] {
            for gap in &gaps {
                let expected =
                    gap.split(';').any(|segment| line_breaks(segment, version).nth(1).is_some());
                assert_eq!(super::run_has_blank(gap, version), expected, "{gap:?} in {version:?}");
            }
        }
    }

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
