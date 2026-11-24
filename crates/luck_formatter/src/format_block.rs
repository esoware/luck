//! Block emission: the statement loop that places the comments between
//! statements and keeps blank lines between them.
//!
//! For each statement the loop writes the comments before it (noting a
//! `luck: ignore`), adds the `;` guard before a leading `(`, and writes the
//! statement and then the comments after it on its line. A `format off`
//! region goes out byte for byte as one verbatim item, and a statement under
//! `luck: ignore` keeps its layout, re-indented to its block.

use std::iter::{self, Peekable};
use std::ops::Range;

use luck_allocator::Allocator;
use luck_ast::{Block, GetSpan as _, Statement, Visit};
use luck_parser::{Groupings, ParseResult};
use luck_token::Span;

use crate::ir::Formatter;
use crate::lines::{
    has_blank_line, has_leading_edge_blank, has_trailing_edge_blank, own_line_start,
};
use crate::sort_requires::{RequireRun, sortable_require};
use crate::{BlockNewlineGaps, Comments, FormatOptions};

#[derive(Debug, Clone, Copy)]
enum BlockItem<'block, 'ast> {
    Stmt(&'block Statement<'ast>),
    Verbatim { span: Span, has_leading_paren: bool },
}

impl BlockItem<'_, '_> {
    fn start(self) -> u32 {
        match self {
            Self::Stmt(stmt) => stmt.span().start,
            Self::Verbatim { span, .. } => span.start,
        }
    }

    /// A `return`'s span takes the `;` after it, which the formatter drops,
    /// so it ends at its last value here, and the parentheses the source
    /// grouped that in, which `grouping` finds: a comment there trails the
    /// values.
    fn span(self, grouping: impl FnOnce(Span) -> Option<Span>) -> Span {
        match self {
            Self::Stmt(Statement::Return(return_stmt)) => {
                let span = return_stmt.span;
                return_stmt.exprs.last().map_or(span, |last| {
                    let end = grouping(last.span()).unwrap_or_else(|| last.span()).end;
                    Span::new(span.start, end)
                })
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
}

/// A parse-error placeholder prints nothing, so it gets no separator either.
fn printed_items<'block, 'ast>(
    block: &'block Block<'ast>,
) -> impl Iterator<Item = BlockItem<'block, 'ast>> + Clone {
    block.stmts.iter().filter(|stmt| !matches!(stmt, Statement::Error(_))).map(BlockItem::Stmt)
}

pub(crate) fn first_item_start(block: &Block<'_>) -> Option<u32> {
    printed_items(block).next().map(BlockItem::start)
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
        first_item_start(block).is_some() || self.comments().has_dangling_comments(block.span.start)
    }

    pub(crate) fn write_block(&mut self, block: &Block<'ast>) {
        let should_preserve_edge_blanks =
            self.options().block_newline_gaps == BlockNewlineGaps::Preserve;
        if should_preserve_edge_blanks
            && first_item_start(block).is_some_and(|start| self.has_blank_line_before(start))
        {
            self.empty_line();
        }

        let Some(last_end) = self.write_items(printed_items(block)) else {
            self.write_dangling_comments(block.span.start);
            return;
        };
        self.write_tail_comments(last_end);

        if should_preserve_edge_blanks {
            let printed_end =
                self.comments().last_printed_end().map_or(last_end, |end| end.max(last_end));
            let version = self.version();
            let has_blank = self
                .comments()
                .source_text()
                .is_some_and(|source| has_trailing_edge_blank(source, printed_end, version));
            if has_blank {
                self.empty_line();
            }
        }
    }

    fn write_items<'block, Items>(&mut self, items: Items) -> Option<u32>
    where
        Items: Iterator<Item = BlockItem<'block, 'ast>> + Clone,
        'ast: 'block,
    {
        let should_sort_requires = self.sorts_requires();
        let mut remaining = items.peekable();
        let mut previous: Option<PreviousItem> = None;
        let mut requires = RequireRun::default();
        while let Some(item) =
            self.next_item(&mut remaining, previous.map_or(0, |last_item| last_item.end))
        {
            let Span { start, end } = item.span(|expr| self.grouping_around(expr));
            let should_surround_with_blank = item.should_surround_with_blank();

            let is_blank_before = previous.is_some_and(|previous_item| {
                self.write_separator(previous_item, start, should_surround_with_blank)
            });

            let before_comments = self.mark();
            let is_opted_out = self.write_stmt_leading_comments(start);
            let has_comment_lines = self.mark() != before_comments;

            // A statement starting with `(` would reparse as a call on the
            // one before it (`f()` then `(g)()` reads as `f()(g)()`), so it
            // gets a `;` prefix, as in the compact printer. A `format off`
            // region can't gain one, so its guard goes on a line above it.
            if previous.is_some() && self.item_starts_with_paren(item, is_opted_out) {
                self.token(";");
                if matches!(item, BlockItem::Verbatim { .. }) {
                    self.hard_line();
                }
            }

            let item_start = self.mark();
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
                    Some(require) => requires.push(require, item_start..self.mark()),
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
        if let Some(span) = items.peek().map(|item| item.span(|expr| self.grouping_around(expr))) {
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
            while let Some(item) = lookahead.next_if(|item| item.start() < run.end) {
                let span = item.span(|expr| self.grouping_around(expr));
                // The region goes out as written, so it starts as its source
                // does, and every node starts at a token.
                if absorbed.is_none() {
                    has_leading_paren = source[span.start as usize] == b'(';
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

        let stays_in_block = match absorbed {
            Some(range) => {
                comments.is_code_free(run.start, range.start)
                    && comments.is_code_free(range.end, run.end)
            }
            None => comments.is_code_free(run.start, run.end),
        };
        if !stays_in_block {
            return items.next();
        }
        *items = lookahead;
        Some(BlockItem::Verbatim { span: Span::new(run.start, run.end), has_leading_paren })
    }

    /// A statement that opted out goes out as its source has it, wherever
    /// there's a source.
    fn item_starts_with_paren(&self, item: BlockItem<'_, 'ast>, is_opted_out: bool) -> bool {
        match item {
            BlockItem::Stmt(stmt) => match self.comments().source_text() {
                Some(source) if is_opted_out => source[stmt.span().start as usize] == b'(',
                Some(_) | None => {
                    stmt.leading_prefix().is_some_and(|prefix| self.starts_with_paren(prefix))
                }
            },
            BlockItem::Verbatim { has_leading_paren, .. } => has_leading_paren,
        }
    }

    /// Whether the source has a blank line right above the node at `start`,
    /// or above the first comment leading it. Synthetic input has no source
    /// and never does.
    pub(crate) fn has_blank_line_before(&self, start: u32) -> bool {
        let comments = self.comments();
        let anchor = comments.peek_next_start().filter(|comment| *comment < start).unwrap_or(start);
        comments
            .source_text()
            .is_some_and(|source| has_leading_edge_blank(source, anchor, self.version()))
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
        let version = self.version();
        let is_blank = match comments.source_text() {
            Some(source) => {
                // Stop at an intervening comment, so a blank line after it isn't
                // counted as part of this gap.
                let gap_end = comments.peek_next_start().unwrap_or(next_start).min(next_start);
                // A line break inside a printed trailing comment isn't a gap line.
                let gap_start = comments
                    .last_printed_end()
                    .map_or(previous.end, |printed_end| printed_end.max(previous.end));
                has_blank_line(source, gap_start, gap_end, version)
            }
            None => {
                previous.should_surround_with_blank
                    || should_surround_next_with_blank
                    || comments.has_synthetic_blank_before(next_start)
            }
        };
        self.line_break(is_blank);
        is_blank
    }

    /// `end` is where the item ends, as [`BlockItem::span`] has it.
    fn write_item(&mut self, item: BlockItem<'_, 'ast>, end: u32) {
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
            self.write_item(item, end);
            return;
        }
        let checkpoint = self.checkpoint();
        self.write_item(item, end);
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
    /// It's split only at the line breaks between tokens, and the statement's
    /// own indentation comes off the lines after its first, so the printer's
    /// indent takes the original column's place.
    ///
    /// Returns false when there's no source to copy.
    fn write_reindented(&mut self, start: u32, end: u32) -> bool {
        let Some(source) = self.comments().source_text() else {
            return false;
        };
        let base_indent = own_line_start(source, start)
            .map_or(&b""[..], |line_start| Span::new(line_start, start).text(source));
        let line_breaks = self.comments().line_breaks_between_tokens(start, end);
        let mut line_start = start;
        for line_break in line_breaks.into_iter().chain(iter::once(Span::new(end, end))) {
            let line = Span::new(line_start, line_break.start).text(source);
            let unindented = if line_start == start {
                line
            } else {
                line.strip_prefix(base_indent).unwrap_or(line)
            };
            if !unindented.is_empty() {
                self.text(unindented);
            }
            if !line_break.is_empty() {
                self.verbatim(line_break.text(source));
            }
            line_start = line_break.end;
        }
        self.comments_mut().mark_printed_range(start, end);
        true
    }
}

struct InnermostBlock<'tree> {
    range: Span,
    groupings: &'tree Groupings,
    found: &'tree Block<'tree>,
}

impl<'tree> Visit<'tree> for InnermostBlock<'tree> {
    fn visit_block(&mut self, block: &'tree Block<'tree>) {
        let first_start = first_item_start(block);
        let last_end = printed_items(block)
            .last()
            .map(|item| item.span(|expr| self.groupings.around(expr)).end);
        if first_start.is_some_and(|start| start <= self.range.start)
            && last_end.is_some_and(|end| self.range.end <= end)
        {
            self.found = block;
            self.walk_block(block);
        }
    }
}

/// Format the statements of `parse`, or of its innermost nested block that
/// holds the whole of `range`, that overlap `range`.
///
/// Returns the source span they replace, from the start of the first one's
/// line to the end of the last one's trailing comment, and the text to put
/// there. `None` when no statement overlaps `range`.
pub(crate) fn format_selection<'ast>(
    allocator: &'ast Allocator,
    parse: &'ast ParseResult<'ast>,
    options: &FormatOptions,
    range: Span,
) -> Option<(Span, Vec<u8>)> {
    let groupings = |expr| parse.groupings.around(expr);
    let block = &parse.chunk.block;
    let mut finder = InnermostBlock { range, groupings: &parse.groupings, found: block };
    finder.visit_block(block);

    let is_selected = |item: &BlockItem<'_, '_>| {
        let Span { start, end } = item.span(groupings);
        end > range.start && start < range.end
    };
    let items = printed_items(finder.found)
        .skip_while(|item| !is_selected(item))
        .take_while(|item| is_selected(item));
    let first_start = items.clone().next()?.start();
    let last_end = items.clone().last()?.span(groupings).end;

    let source = parse.source;
    let line_start = own_line_start(source, first_start).unwrap_or(first_start);
    let base_indent = Span::new(line_start, first_start).text(source);
    let mut comments = Comments::from_parse(parse);
    comments.restrict_to(Span::new(line_start, last_end));

    let mut f =
        Formatter::new(allocator, parse.chunk.version, *options, comments, Some(&parse.groupings));
    f.write_items(items);
    let end = f.comments().last_printed_end().map_or(last_end, |end| end.max(last_end));
    Some((Span::new(line_start, end), f.print(base_indent)))
}
