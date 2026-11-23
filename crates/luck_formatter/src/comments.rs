//! Parsed source carries comments as a sorted `[Comment]`, and a synthetic AST
//! carries `SyntheticComment`s keyed by their anchor node's span start.
//!
//! The sourced store is a cursor over the sorted array, as `oxc_formatter`'s
//! is. Every pass takes a run from the front of what's left, so comments print
//! in source order and once each, and each lands at the nearest node boundary:
//!
//! - A node's leading comments are all those before it.
//! - Its trailing comments are those right after it on its line, before the
//!   end of the node around it.
//! - A container takes those before its closer, and a block those around its
//!   statements.
//!
//! A parsed comment's text is sliced from the source as it's written, and only
//! one whose line breaks need rewriting is copied, to the arena.
//!
//! The synthetic store is a map keyed by anchor: synthesis order needn't match
//! document order, and a synthesizer may comment every statement. It also holds
//! the anchors that want a blank line before them
//! ([`Comments::with_blank_before`]), since there's no source to find blank
//! lines in. Only statements and blocks take synthetic comments.

use alloc::collections::BTreeMap;
use core::iter::once;
use core::mem::take;
use core::ops::Range;

use luck_allocator::Allocator;
use luck_ast::SyntheticComment;
use luck_token::{
    Comment, CommentContent, CommentKind, CommentPosition, LuaVersion, Span, has_line_break,
    index_to_offset, is_lua_whitespace, line_breaks, offset_to_index, strip_line_break,
};
use rustc_hash::FxHashSet;

use crate::format_block::{has_blank_line, has_leading_edge_blank};
use crate::ir::{Formatter, LineMode};

#[derive(Debug)]
struct Sourced<'ast> {
    comments: &'ast [Comment],
    source: &'ast str,
    printed_count: usize,
    /// The spans `-- luck: format off` / `on` regions copy as written,
    /// the directive comments and their own lines included. Sorted, and
    /// empty on almost every file.
    disabled_ranges: Vec<Range<u32>>,
}

#[derive(Debug)]
enum Store<'ast> {
    Sourced(Sourced<'ast>),
    Synthetic {
        /// Keyed by anchor and drained as each anchor is formatted. Whatever
        /// is left flushes at end of file in anchor order.
        comments: BTreeMap<u32, Vec<SyntheticComment<'ast>>>,
        blank_before_anchors: FxHashSet<u32>,
    },
    Empty,
}

/// The comments [`format_chunk`](crate::format_chunk()) places around a tree.
#[derive(Debug)]
pub struct Comments<'ast> {
    store: Store<'ast>,
    /// Where the first unprinted parsed comment starts, or `u32::MAX` past
    /// the last, kept beside the store because every node checks it on its
    /// way in and out.
    next_start: u32,
}

/// Only the parsed path's cursor rewinds. The one speculative region, a
/// statement that may hold a `format off` region, needs source, and a
/// synthetic comment is taken once.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CommentsCheckpoint {
    printed_count: usize,
}

impl<'ast> Comments<'ast> {
    #[must_use]
    pub const fn none() -> Self {
        Self { store: Store::Empty, next_start: u32::MAX }
    }

    pub(crate) fn from_source(
        comments: &'ast [Comment],
        source: &'ast str,
        version: LuaVersion,
    ) -> Self {
        let region_start = |off: &Comment| {
            own_line_start(source, off.span.start)
                .unwrap_or_else(|| after_line_break(source, off.span.end, version))
        };
        // An `off` inside an open region changes nothing.
        let mut disabled_ranges = Vec::new();
        let mut open_off: Option<&Comment> = None;
        for comment in comments {
            match comment.content {
                CommentContent::FormatOff => {
                    open_off.get_or_insert(comment);
                }
                CommentContent::FormatOn => {
                    if let Some(off) = open_off.take() {
                        disabled_ranges.push(region_start(off)..comment.span.end);
                    }
                }
                CommentContent::Plain
                | CommentContent::HotComment
                | CommentContent::FormatIgnore
                | CommentContent::LintDirective
                | CommentContent::InvalidDirective => {}
            }
        }
        if let Some(off) = open_off {
            disabled_ranges.push(region_start(off)..index_to_offset(source.len()));
        }
        let sourced = Sourced { comments, source, printed_count: 0, disabled_ranges };
        let next_start = sourced.next_start();
        Self { store: Store::Sourced(sourced), next_start }
    }

    #[must_use]
    pub fn synthetic<Synthetic>(comments: Synthetic) -> Self
    where
        Synthetic: IntoIterator<Item = SyntheticComment<'ast>>,
    {
        let mut comments_by_anchor: BTreeMap<u32, Vec<SyntheticComment<'ast>>> = BTreeMap::new();
        for comment in comments {
            comments_by_anchor.entry(comment.attached_to).or_default().push(comment);
        }
        Self {
            store: Store::Synthetic {
                comments: comments_by_anchor,
                blank_before_anchors: FxHashSet::default(),
            },
            next_start: u32::MAX,
        }
    }

    /// Request a blank line before each statement whose span start is in
    /// `anchors`.
    #[must_use]
    pub fn with_blank_before<Anchors>(mut self, anchors: Anchors) -> Self
    where
        Anchors: IntoIterator<Item = u32>,
    {
        if matches!(self.store, Store::Empty) {
            self.store = Store::Synthetic {
                comments: BTreeMap::new(),
                blank_before_anchors: FxHashSet::default(),
            };
        }
        if let Store::Synthetic { blank_before_anchors, .. } = &mut self.store {
            blank_before_anchors.extend(anchors);
        }
        self
    }

    pub(crate) fn has_synthetic_blank_before(&self, anchor: u32) -> bool {
        match &self.store {
            Store::Synthetic { blank_before_anchors, .. } => blank_before_anchors.contains(&anchor),
            Store::Sourced(_) | Store::Empty => false,
        }
    }

    pub(crate) const fn source_text(&self) -> Option<&'ast str> {
        match &self.store {
            Store::Sourced(sourced) => Some(sourced.source),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    /// The first `format off` region not yet behind `position` or the last
    /// printed comment.
    pub(crate) fn next_disabled_region(&self, position: u32) -> Option<Range<u32>> {
        let Store::Sourced(sourced) = &self.store else {
            return None;
        };
        if sourced.disabled_ranges.is_empty() {
            return None;
        }
        let reached = sourced.printed_end().map_or(position, |end| end.max(position));
        region_ending_after(&sourced.disabled_ranges, reached).cloned()
    }

    pub(crate) fn overlaps_disabled_region(&self, span: Span) -> bool {
        match &self.store {
            Store::Sourced(sourced) => region_ending_after(&sourced.disabled_ranges, span.start)
                .is_some_and(|region| region.start < span.end),
            Store::Synthetic { .. } | Store::Empty => false,
        }
    }

    /// Whether `from..to` holds nothing but comments, whitespace and `;`.
    pub(crate) fn is_free_of_tokens(&self, from: u32, to: u32) -> bool {
        let Store::Sourced(Sourced { comments, source, .. }) = &self.store else {
            return false;
        };
        let first_index = comments.partition_point(|c| c.span.start < from);
        let within = comments.get(first_index..).unwrap_or_default();
        let mut gap_start = from;
        for comment in within.iter().take_while(|c| c.span.start < to) {
            if !is_code_free(Span::new(gap_start, comment.span.start).text(source)) {
                return false;
            }
            gap_start = comment.span.end;
        }
        gap_start >= to || is_code_free(Span::new(gap_start, to).text(source))
    }

    /// Whether an unprinted comment starts in `start..end`.
    ///
    /// Before the stretch is written, as a layout that has to know what's
    /// there asks, that's any comment in it, and the other questions about a
    /// stretch are asked then too. After, it's one a `format off` region in
    /// it held back.
    pub(crate) fn has_comment_in(&self, start: u32, end: u32) -> bool {
        self.unprinted_from(start).first().is_some_and(|c| c.span.start < end)
    }

    /// Whether a comment in `span` lies outside every one of `items`, which
    /// are in order and inside `span`, for a layout that drops the delimiters
    /// around them.
    pub(crate) fn has_comment_outside(
        &self,
        span: Span,
        items: impl Iterator<Item = Span>,
    ) -> bool {
        self.comments_outside(span, items).next().is_some()
    }

    /// Whether a comment in `span` that ends its line lies outside every one
    /// of `items`, which are in order and inside `span`. That comment breaks
    /// lines a layout that hugs has no room for.
    pub(crate) fn has_line_ending_comment_outside(
        &self,
        span: Span,
        items: impl Iterator<Item = Span>,
    ) -> bool {
        self.comments_outside(span, items).any(ends_line)
    }

    fn comments_outside(
        &self,
        span: Span,
        items: impl Iterator<Item = Span>,
    ) -> impl Iterator<Item = &'ast Comment> {
        let within = self.unprinted_from(span.start);
        let mut remaining_items = items.peekable();
        within.iter().take_while(move |c| c.span.start < span.end).filter(move |comment| {
            while remaining_items.next_if(|item| item.end <= comment.span.start).is_some() {}
            remaining_items.peek().is_none_or(|item| item.start > comment.span.start)
        })
    }

    /// The unprinted comments from the first at or after `start` on.
    fn unprinted_from(&self, start: u32) -> &'ast [Comment] {
        match &self.store {
            Store::Sourced(sourced) => sourced.pending_from(start),
            Store::Synthetic { .. } | Store::Empty => &[],
        }
    }

    /// Where the first `closer` byte at or after `from` that no comment holds
    /// is, which the tree doesn't record for every closer. Only comments,
    /// whitespace and punctuation may come first, so no string can hold it.
    pub(crate) fn closer_offset(&self, from: u32, closer: u8) -> u32 {
        let Some(source) = self.source_text() else {
            return from;
        };
        let source_end = index_to_offset(source.len());
        let after = self.unprinted_from(from);
        // The stretches between the comments, and the one after the last.
        let starts = once(from).chain(after.iter().map(|c| c.span.end));
        let ends = after.iter().map(|c| c.span.start).chain(once(source_end));
        starts
            .zip(ends)
            .find_map(|(start, end)| {
                let i = Span::new(start, end).text(source).bytes().position(|b| b == closer)?;
                Some(start.saturating_add(index_to_offset(i)))
            })
            .unwrap_or(source_end)
    }

    /// Narrow this store to `selection`: comments before it count as printed,
    /// and no `format off` region reaches outside it.
    pub(crate) fn restrict_to(&mut self, selection: Span) {
        if let Store::Sourced(sourced) = &mut self.store {
            let printed_count =
                sourced.comments.partition_point(|c| c.span.start < selection.start);
            for region in &mut sourced.disabled_ranges {
                region.start = region.start.max(selection.start);
                region.end = region.end.min(selection.end);
            }
            sourced.disabled_ranges.retain(|region| region.start < region.end);
            self.set_printed_count(printed_count);
        }
    }

    /// Span start of the next unprinted comment (parsed path only), which
    /// blank-line logic uses to bound its gap scans.
    pub(crate) fn peek_next_start(&self) -> Option<u32> {
        Some(self.next_start).filter(|start| *start != u32::MAX)
    }

    /// Whether an unprinted comment starts before `position`, which every
    /// node asks on its way in and out.
    #[inline]
    pub(crate) const fn has_pending_before(&self, position: u32) -> bool {
        self.next_start < position
    }

    /// Takes the first `n` unprinted comments.
    fn take(&mut self, n: usize) -> &'ast [Comment] {
        let Store::Sourced(sourced) = &mut self.store else {
            return &[];
        };
        let taken = sourced.take(n);
        self.next_start = sourced.next_start();
        taken
    }

    fn set_printed_count(&mut self, printed_count: usize) {
        if let Store::Sourced(sourced) = &mut self.store {
            sourced.printed_count = printed_count;
            self.next_start = sourced.next_start();
        }
    }

    pub(crate) fn last_printed_end(&self) -> Option<u32> {
        match &self.store {
            Store::Sourced(sourced) => sourced.printed_end(),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    /// Whether a comment the node at `start` leads is next to print.
    #[inline]
    pub(crate) fn has_comment_before(&self, start: u32) -> bool {
        self.next_start < start && self.placeable_before(start).next().is_some()
    }

    /// Whether a comment the node at `start` leads ends its line, which moves
    /// that node to a line of its own.
    #[inline]
    pub(crate) fn has_line_ending_comment_before(&self, start: u32) -> bool {
        self.next_start < start && self.placeable_before(start).any(ends_line)
    }

    fn placeable_before(&self, position: u32) -> impl Iterator<Item = &'ast Comment> + use<'ast> {
        let placeable = match &self.store {
            Store::Sourced(sourced) => Some(sourced.placeable_before(position)),
            Store::Synthetic { .. } | Store::Empty => None,
        };
        placeable.into_iter().flatten()
    }

    /// Whether a block with no statements, whose span starts at `block_start`,
    /// has comments to print.
    ///
    /// On the parsed path those are the [`dangling_run`] there; on the
    /// synthetic path, any comment anchored at `block_start`.
    pub(crate) fn has_dangling_comments(&self, block_start: u32) -> bool {
        match &self.store {
            Store::Sourced(sourced) => {
                let limit = sourced.disabled_region_limit();
                !dangling_run(sourced.pending(), sourced.source, block_start, limit).is_empty()
            }
            Store::Synthetic { comments, .. } => comments.contains_key(&block_start),
            Store::Empty => false,
        }
    }

    /// Mark the comments in `start..end` printed without emitting them, for a
    /// statement whose verbatim slice already holds them.
    pub(crate) fn mark_printed_range(&mut self, start: u32, end: u32) {
        if let Store::Sourced(sourced) = &self.store {
            let pending = sourced.pending();
            let count = pending.iter().take_while(|c| (start..end).contains(&c.span.start)).count();
            self.take(count);
        }
    }

    pub(crate) const fn checkpoint(&self) -> CommentsCheckpoint {
        CommentsCheckpoint {
            printed_count: match &self.store {
                Store::Sourced(sourced) => sourced.printed_count,
                Store::Synthetic { .. } | Store::Empty => 0,
            },
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: CommentsCheckpoint) {
        self.set_printed_count(checkpoint.printed_count);
    }

    /// The parsed path's store, whose comments and source outlive it.
    const fn sourced_mut(&mut self) -> Option<&mut Sourced<'ast>> {
        if let Store::Sourced(sourced) = &mut self.store { Some(sourced) } else { None }
    }
}

impl<'ast> Sourced<'ast> {
    fn pending(&self) -> &'ast [Comment] {
        self.comments.get(self.printed_count..).unwrap_or_default()
    }

    fn printed_end(&self) -> Option<u32> {
        let last_index = self.printed_count.checked_sub(1)?;
        self.comments.get(last_index).map(|c| c.span.end)
    }

    /// Where the next `format off` region starts, past which no pass may take
    /// a comment, since the region's own bytes hold it.
    fn disabled_region_limit(&self) -> u32 {
        if self.disabled_ranges.is_empty() {
            return u32::MAX;
        }
        let position = self.printed_end().unwrap_or(0);
        region_ending_after(&self.disabled_ranges, position).map_or(u32::MAX, |region| region.start)
    }

    fn next_start(&self) -> u32 {
        self.pending().first().map_or(u32::MAX, |c| c.span.start)
    }

    /// The pending comments before `position` a pass may take, read lazily,
    /// since a pass usually takes the first few.
    fn placeable_before(&self, position: u32) -> impl Iterator<Item = &'ast Comment> + use<'ast> {
        let limit = self.disabled_region_limit().min(position);
        self.pending().iter().take_while(move |c| c.span.start < limit)
    }

    /// The pending comments from the first at or after `start` on. It's
    /// nearly always the first pending one, since only the leading comments of
    /// what's about to be written, or a `format off` region, hold the cursor
    /// back.
    fn pending_from(&self, start: u32) -> &'ast [Comment] {
        let pending = self.pending();
        if pending.first().is_none_or(|c| c.span.start >= start) {
            return pending;
        }
        pending.get(pending.partition_point(|c| c.span.start < start)..).unwrap_or_default()
    }

    /// The source from the first token at or after `offset`, past whitespace
    /// and the pending comments.
    fn next_token(&self, offset: u32) -> &'ast [u8] {
        let bytes = self.source.as_bytes();
        let mut position = offset_to_index(offset);
        let mut comments = self.pending_from(offset);
        loop {
            let rest = bytes.get(position..).unwrap_or_default();
            let whitespace = rest.iter().take_while(|b| is_lua_whitespace(**b)).count();
            position = position.saturating_add(whitespace);
            match comments.split_first() {
                Some((comment, after)) if offset_to_index(comment.span.start) == position => {
                    position = offset_to_index(comment.span.end);
                    comments = after;
                }
                Some(_) | None => return bytes.get(position..).unwrap_or_default(),
            }
        }
    }

    /// Takes the first `n` pending comments, which only
    /// [`Comments::take`] calls, since it keeps `next_start` up to date.
    fn take(&mut self, n: usize) -> &'ast [Comment] {
        let pending = self.pending();
        self.printed_count = self.printed_count.saturating_add(n);
        pending.get(..n).unwrap_or_default()
    }
}

impl<'ast> Formatter<'ast> {
    /// The text a parsed comment prints as, with its line breaks written as
    /// LF.
    fn sourced_text(&mut self, comment: &Comment) -> &'ast str {
        let Some(source) = self.comments().source_text() else {
            return "";
        };
        let version = self.version();
        let text = comment.span.text(source);
        if comment.kind == CommentKind::Shebang {
            // Loaders skip this line up to LF, so a CR in it is text, but one
            // the printer leaves at its end, past the spaces and tabs it
            // trims, would pair with the line break written after it.
            return text.trim_end_matches(['\r', ' ', '\t']);
        }
        // Only a block comment the lexer saw a line break in holds one.
        if comment.kind != CommentKind::MultiLineBlock || !text.contains('\r') {
            return text;
        }
        let (normalized, allocator) = self.scratch();
        let mut cursor = 0;
        for line_break in line_breaks(text, version) {
            // Luau reads a lone CR as a space, and one left before the LF
            // would make a CRLF of it.
            let line = text.get(cursor..line_break.start).unwrap_or_default();
            normalized.push_str(line.trim_end_matches('\r'));
            normalized.push('\n');
            cursor = line_break.end;
        }
        normalized.push_str(text.get(cursor..).unwrap_or_default());
        allocator.alloc_str(normalized)
    }

    /// Writes the comments before `start`, which lead the node there.
    ///
    /// A comment followed by code on its line stays on the line of what
    /// follows it, one that ends its line breaks it, and one alone on its line
    /// keeps a line of its own. Returns true when one of them is a
    /// `luck: ignore` directive.
    #[inline]
    pub(crate) fn write_leading_comments(&mut self, start: u32) -> bool {
        self.comments().has_pending_before(start) && self.write_leading_run(start)
    }

    fn write_leading_run(&mut self, start: u32) -> bool {
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return false;
        };
        let count = sourced.placeable_before(start).count();
        if count == 0 {
            return false;
        }
        let source = sourced.source;
        let version = self.version();
        let taken = self.comments_mut().take(count);
        let mut has_ignore = false;
        let followers = taken.iter().skip(1).map(|c| c.span.start).chain(once(start));
        for (comment, next_start) in taken.iter().zip(followers) {
            let is_line_ending = ends_line(comment);
            if is_line_ending && starts_line(source, comment.span.start, version) {
                self.hard_line();
            }
            has_ignore |= comment.content == CommentContent::FormatIgnore;
            let text = self.sourced_text(comment);
            self.text(text);
            if is_line_ending {
                self.line_break(has_blank_line(source, comment.span.end, next_start, version));
            } else {
                self.space();
            }
        }
        has_ignore
    }

    /// Writes the leading comments of the operand at `start` ahead of the
    /// operator before it, which starts the operand's line when the chain
    /// breaks, when one of them ends its line. A comment that doesn't stays
    /// with the operand, after the operator.
    pub(crate) fn write_hoisted_comments(&mut self, start: u32) {
        if self.comments().has_line_ending_comment_before(start) {
            self.write_leading_comments(start);
        }
    }

    /// Writes the comments right after a node ending at `end`, on its line and
    /// before `enclosing_end`, where the node around it ends.
    ///
    /// The ones before the first separator trail the node in place. When the
    /// run reaches a comment that ends the line, that one and those between
    /// it and the separators before it go to the line's end, which breaks the
    /// groups around it. The rest lead what follows.
    ///
    /// Comments that call arguments or parameters follow lead those instead.
    /// In Lua 5.1 and Luau a line break before a call's `(` makes a new
    /// statement of it, and one moved past a function's `(...)` would lead its
    /// body on the next format.
    #[inline]
    pub(crate) fn write_trailing_comments(&mut self, end: u32, enclosing_end: u32) {
        if self.comments().has_pending_before(enclosing_end) {
            self.write_trailing_run(end, enclosing_end);
        }
    }

    fn write_trailing_run(&mut self, end: u32, enclosing_end: u32) {
        let version = self.version();
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return;
        };
        let source = sourced.source;
        let mut gap_start = end;
        let mut in_place_count = 0usize;
        let mut run_length = 0usize;
        let mut in_place_end = end;
        let mut has_separator = false;
        let mut is_line_ending = false;
        for comment in sourced.placeable_before(enclosing_end) {
            if comment.span.start < gap_start {
                return;
            }
            let gap = Span::new(gap_start, comment.span.start).text(source);
            if !gap.bytes().all(is_separator_gap_byte) || has_line_break(gap, version) {
                break;
            }
            has_separator |= gap.bytes().any(|b| matches!(b, b',' | b';'));
            run_length = run_length.saturating_add(1);
            gap_start = comment.span.end;
            if ends_line(comment) {
                is_line_ending = true;
                break;
            }
            if !has_separator {
                in_place_count = run_length;
                in_place_end = gap_start;
            }
        }
        let (count, last_end) =
            if is_line_ending { (run_length, gap_start) } else { (in_place_count, in_place_end) };
        if count == 0 || (!has_separator && starts_call_arguments(sourced.next_token(last_end))) {
            return;
        }
        let taken = self.comments_mut().take(count);
        for (i, comment) in taken.iter().enumerate() {
            let text = self.sourced_text(comment);
            if i < in_place_count {
                self.space();
                self.text(text);
            } else {
                self.line_suffix(text, comment.kind != CommentKind::Line);
            }
        }
        if is_line_ending {
            self.expand_parent();
        }
    }

    /// Writes the comments before a container's closer at `closer`, after its
    /// last item. Each keeps the line it starts or the place on its line, and
    /// a line comment ends the line it's on.
    #[inline]
    pub(crate) fn write_closing_comments(&mut self, closer: u32, keeps_blank_lines: bool) {
        if self.comments().has_pending_before(closer) {
            self.write_closing_run(closer, keeps_blank_lines);
        }
    }

    fn write_closing_run(&mut self, closer: u32, keeps_blank_lines: bool) {
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return;
        };
        let count = sourced.placeable_before(closer).count();
        if count == 0 {
            return;
        }
        let source = sourced.source;
        let version = self.version();
        let mut has_ended_line = false;
        for comment in self.comments_mut().take(count) {
            match (starts_line(source, comment.span.start, version), has_ended_line) {
                (true, _) => {
                    let is_blank = keeps_blank_lines
                        && has_leading_edge_blank(source, comment.span.start, version);
                    self.line_break(is_blank);
                }
                (false, false) => self.space(),
                // The line comment before it ended the line it was on.
                (false, true) => {}
            }
            let text = self.sourced_text(comment);
            self.text(text);
            has_ended_line = comment.kind == CommentKind::Line;
            if has_ended_line {
                self.hard_line();
            }
        }
    }

    /// Writes `open`, the comments before `closer`, and `close`, for a
    /// container with no items, in a group that breaks around them like the
    /// container's items would. A line comment breaks it.
    pub(crate) fn write_dangling_list(
        &mut self,
        delimiters: (&'static str, &'static str),
        closer: u32,
        line: LineMode,
    ) {
        let (open, close) = delimiters;
        let comments = self.comments_mut();
        let taken = if comments.has_pending_before(closer) {
            let count = comments.placeable_before(closer).count();
            comments.take(count)
        } else {
            &[]
        };
        let Some(source) = self.comments().source_text().filter(|_| !taken.is_empty()) else {
            self.token(open);
            self.token(close);
            return;
        };
        let version = self.version();
        self.group(|grouped| {
            grouped.token(open);
            grouped.indent(|f| {
                f.line(line);
                let mut previous: Option<&Comment> = None;
                for comment in taken {
                    if let Some(before) = previous {
                        if before.kind == CommentKind::Line
                            || starts_line(source, comment.span.start, version)
                        {
                            let gap_start = before.span.end;
                            f.line_break(has_blank_line(
                                source,
                                gap_start,
                                comment.span.start,
                                version,
                            ));
                        } else {
                            f.space();
                        }
                    }
                    let text = f.sourced_text(comment);
                    f.text(text);
                    if comment.kind == CommentKind::Line {
                        f.expand_parent();
                    }
                    previous = Some(comment);
                }
            });
            grouped.line(line);
            grouped.token(close);
        });
    }

    /// Writes the comments that lead a statement starting at `anchor`.
    ///
    /// Returns true when one of them is a `luck: ignore` directive, which only
    /// a parsed comment can be, since a tree built in code has no source to
    /// keep a statement as written from.
    pub(crate) fn write_stmt_leading_comments(&mut self, anchor: u32) -> bool {
        if self.comments().source_text().is_some() {
            return self.write_leading_comments(anchor);
        }
        let lines =
            self.take_synthetic(anchor, |comment| comment.position == CommentPosition::Leading);
        for text in lines {
            self.text(text);
            self.hard_line();
        }
        false
    }

    /// Writes the comments after a statement on its line, at the line's end,
    /// which breaks the groups around the statement, as a collapsed `if`.
    ///
    /// Returns true when one of them is a block comment spanning lines, which
    /// ends its line somewhere other than where the statement's does.
    pub(crate) fn write_stmt_trailing_comments(&mut self, anchor: u32, stmt_end: u32) -> bool {
        let version = self.version();
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            let texts = self
                .take_synthetic(anchor, |comment| comment.position == CommentPosition::Trailing);
            for text in &texts {
                self.line_suffix(text, is_long_comment(text));
            }
            if !texts.is_empty() {
                self.expand_parent();
            }
            return false;
        };
        let source = sourced.source;
        let mut gap_start = stmt_end;
        // Luau ends a line comment at a lone CR but reads no line break
        // there, so a comment after one is on the same line, where it can't
        // print without joining it.
        let mut is_after_line_comment = false;
        let count = sourced
            .placeable_before(u32::MAX)
            .take_while(|comment| {
                if is_after_line_comment || gap_start > comment.span.start {
                    return false;
                }
                let gap = Span::new(gap_start, comment.span.start).text(source);
                gap_start = comment.span.end;
                is_after_line_comment = comment.kind == CommentKind::Line;
                is_code_free(gap) && !has_line_break(gap, version)
            })
            .count();
        if count == 0 {
            return false;
        }
        let taken = self.comments_mut().take(count);
        for comment in taken {
            let text = self.sourced_text(comment);
            self.line_suffix(text, comment.kind != CommentKind::Line);
        }
        self.expand_parent();
        taken.iter().any(|comment| comment.kind == CommentKind::MultiLineBlock)
    }

    /// Writes `comments` a line each with the line breaks between them, but
    /// none after the last.
    fn write_comment_run_from(&mut self, comments: &[Comment]) {
        let Some(source) = self.comments().source_text() else {
            return;
        };
        let version = self.version();
        let mut previous_end = None;
        for comment in comments {
            if let Some(end) = previous_end {
                self.line_break(has_blank_line(source, end, comment.span.start, version));
            }
            previous_end = Some(comment.span.end);
            let text = self.sourced_text(comment);
            self.text(text);
        }
    }

    /// The synthetic comments anchored at `anchor` that `filter` takes, each
    /// line prefixed with `--` where it's missing.
    fn take_synthetic(
        &mut self,
        anchor: u32,
        filter: impl Fn(&SyntheticComment<'_>) -> bool,
    ) -> Vec<&'ast str> {
        let allocator = self.allocator();
        let Store::Synthetic { comments, .. } = &mut self.comments_mut().store else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        if let Some(anchored_comments) = comments.get_mut(&anchor) {
            anchored_comments.retain(|comment| {
                if filter(comment) {
                    synthetic_comment_lines(comment.text, allocator, &mut taken);
                    false
                } else {
                    true
                }
            });
            if anchored_comments.is_empty() {
                comments.remove(&anchor);
            }
        }
        taken
    }

    pub(crate) fn write_shebang(&mut self) {
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return;
        };
        let count = sourced.pending().iter().take_while(|c| c.kind == CommentKind::Shebang).count();
        for comment in self.comments_mut().take(count) {
            let text = self.sourced_text(comment);
            self.text(text);
            self.hard_line();
        }
    }

    /// Write the comments in a block with no statements, whose span starts at
    /// `block_start`, one per line.
    ///
    /// On the parsed path that's the [`dangling_run`] there; on the synthetic
    /// path, every comment anchored at `block_start`. The caller writes the
    /// line breaks around them.
    pub(crate) fn write_dangling_comments(&mut self, block_start: u32) {
        if let Some(sourced) = self.comments_mut().sourced_mut() {
            let limit = sourced.disabled_region_limit();
            let count = dangling_run(sourced.pending(), sourced.source, block_start, limit).len();
            let taken = self.comments_mut().take(count);
            self.write_comment_run_from(taken);
            return;
        }
        let lines = self.take_synthetic(block_start, |_| true);
        for (i, text) in lines.into_iter().enumerate() {
            if i > 0 {
                self.hard_line();
            }
            self.text(text);
        }
    }

    /// Write the comments after a block's last statement, each on its own
    /// line and keeping the blank lines around them.
    ///
    /// That's the [`dangling_run`] after `last_end`, so a comment only moves
    /// past whitespace and `;`. Parsed input only: a synthetic comment names
    /// its anchor.
    pub(crate) fn write_tail_comments(&mut self, last_end: u32) {
        let version = self.version();
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return;
        };
        let source = sourced.source;
        let limit = sourced.disabled_region_limit();
        let gap_start = sourced.printed_end().map_or(last_end, |end| end.max(last_end));
        let count = dangling_run(sourced.pending(), source, gap_start, limit).len();
        let taken = self.comments_mut().take(count);
        let Some(first) = taken.first() else {
            return;
        };
        self.line_break(has_blank_line(source, gap_start, first.span.start, version));
        self.write_comment_run_from(taken);
    }

    pub(crate) fn write_remaining_comments(&mut self, has_stmts_before: bool) {
        let allocator = self.allocator();
        let store = self.comments_mut();
        let texts: Vec<&'ast str> = match &mut store.store {
            Store::Sourced(sourced) => {
                let count = sourced.pending().len();
                let taken = store.take(count);
                debug_assert!(taken.is_empty(), "a comment with no place to go: {taken:?}");
                for (i, comment) in taken.iter().enumerate() {
                    if i > 0 || has_stmts_before {
                        self.hard_line();
                    }
                    let text = self.sourced_text(comment);
                    self.text(text);
                }
                return;
            }
            Store::Synthetic { comments, .. } => {
                let mut lines = Vec::new();
                for comment in take(comments).into_values().flatten() {
                    synthetic_comment_lines(comment.text, allocator, &mut lines);
                }
                lines
            }
            Store::Empty => return,
        };
        for (i, text) in texts.into_iter().enumerate() {
            if i > 0 || has_stmts_before {
                self.hard_line();
            }
            self.text(text);
        }
    }
}

/// Whether `comment` ends its line, so nothing may follow it there.
fn ends_line(comment: &Comment) -> bool {
    comment.kind == CommentKind::Line || comment.is_followed_by_newline
}

/// Whether only whitespace comes before `offset` on its line, as `version`
/// breaks lines.
fn starts_line(source: &str, offset: u32, version: LuaVersion) -> bool {
    let before = source.as_bytes().get(..offset_to_index(offset)).unwrap_or_default();
    let is_line_space = |b: &&u8| {
        matches!(b, b' ' | b'\t' | 0x0B | 0x0C) || (**b == b'\r' && !version.has_lfcr_line_break())
    };
    before.iter().rev().find(|b| !is_line_space(b)).is_none_or(|b| matches!(b, b'\n' | b'\r'))
}

/// Whether `token`, a source from a token on, starts call arguments or
/// parameters: a `(`, a table, or a string.
const fn starts_call_arguments(token: &[u8]) -> bool {
    matches!(token, [b'(' | b'{' | b'"' | b'\'', ..] | [b'[', b'[' | b'=', ..])
}

/// The bytes a trailing comment may cross after its node: whitespace and the
/// separators the formatter writes itself.
const fn is_separator_gap_byte(byte: u8) -> bool {
    is_lua_whitespace(byte) || matches!(byte, b',' | b';')
}

/// The first of `regions` that ends past `position`. `format off` regions are
/// sorted and apart, so their ends are in order too.
fn region_ending_after(regions: &[Range<u32>], position: u32) -> Option<&Range<u32>> {
    regions.get(regions.partition_point(|region| region.end <= position))
}

/// The comments a block may print with no statement left to attach them to.
///
/// That's those pending before `start`, where the block's remaining text
/// begins, or none if any can't reach it, then the run after it up to the next
/// token.
///
/// `start` is the closer of an empty block or the end of its last
/// statement.
///
/// Only whitespace and `;` may separate the run's comments from each other and
/// from `start`. The run stops at `limit`.
fn dangling_run<'comments>(
    pending: &'comments [Comment],
    source: &str,
    start: u32,
    limit: u32,
) -> &'comments [Comment] {
    let before_count = pending.iter().take_while(|c| c.span.start < start).count();
    let (before, after) = pending.split_at(before_count);
    if !is_movable_run(before, source, start) {
        return &[];
    }
    let mut gap_start = start;
    let after_count = after
        .iter()
        .take_while(|comment| {
            let is_reachable = comment.span.start < limit
                && is_code_free(Span::new(gap_start, comment.span.start).text(source));
            gap_start = comment.span.end;
            is_reachable
        })
        .count();
    pending.get(..before_count.saturating_add(after_count)).unwrap_or_default()
}

/// Whether every comment of `run` can reach the next one, and the last can
/// reach `run_end`, crossing only what [`is_code_free`] allows.
fn is_movable_run(run: &[Comment], source: &str, run_end: u32) -> bool {
    let followers = run.iter().skip(1).map(|c| c.span.start).chain(once(run_end));
    run.iter().zip(followers).all(|(comment, next_start)| {
        next_start >= comment.span.end
            && is_code_free(Span::new(comment.span.end, next_start).text(source))
    })
}

/// Whether a stretch of source holds no token, so a comment can cross it and
/// still sit between the same two tokens.
///
/// `;` doesn't count, since the formatter writes statement separators as it
/// sees fit.
pub(crate) fn is_code_free(text: &str) -> bool {
    text.bytes().all(is_code_free_byte)
}

pub(crate) const fn is_code_free_byte(byte: u8) -> bool {
    is_lua_whitespace(byte) || byte == b';'
}

/// `offset`'s line's start, when only spaces and tabs come before `offset` on
/// it.
pub(crate) fn own_line_start(source: &str, offset: u32) -> Option<u32> {
    let i = offset_to_index(offset);
    let before = source.as_bytes().get(..i).unwrap_or_default();
    let indent_length = before.iter().rev().take_while(|b| matches!(b, b' ' | b'\t')).count();
    let line_start = i.saturating_sub(indent_length);
    let is_line_start =
        line_start == 0 || matches!(before.get(line_start.saturating_sub(1)), Some(b'\n' | b'\r'));
    is_line_start.then(|| index_to_offset(line_start))
}

fn after_line_break(source: &str, offset: u32, version: LuaVersion) -> u32 {
    let rest = source.as_bytes().get(offset_to_index(offset)..).unwrap_or_default();
    let remaining = strip_line_break(rest, version).map_or(rest.len(), <[u8]>::len);
    index_to_offset(source.len().saturating_sub(remaining))
}

/// Synthetic text may leave out the `--` marker, so each line gets one when
/// missing. A long comment carries its line breaks as content and stays whole.
fn synthetic_comment_lines<'ast>(
    text: &'ast str,
    allocator: &'ast Allocator,
    lines: &mut Vec<&'ast str>,
) {
    if is_long_comment(text) {
        lines.push(text);
        return;
    }
    for line in text.lines().flat_map(|line| line.split('\r')) {
        lines.push(if line.trim_start().starts_with("--") {
            line
        } else if line.is_empty() {
            "--"
        } else {
            allocator.alloc_concat_strs_array(["-- ", line])
        });
    }
}

fn is_long_comment(text: &str) -> bool {
    text.trim_start()
        .strip_prefix("--[")
        .is_some_and(|rest| rest.trim_start_matches('=').starts_with('['))
}

#[cfg(test)]
mod tests {
    use core::iter;

    use super::*;
    use crate::FormatOptions;
    use crate::ir::FormatElement;

    fn make_comment(start: u32, end: u32, kind: CommentKind) -> Comment {
        Comment {
            span: Span::new(start, end),
            attached_to: 0,
            kind,
            position: CommentPosition::Leading,
            is_followed_by_newline: true,
            content: CommentContent::Plain,
        }
    }

    fn texts<'ast>(f: &Formatter<'ast>) -> Vec<&'ast str> {
        f.elements()
            .iter()
            .filter_map(
                |element| {
                    if let FormatElement::Text(text, _) = element { Some(*text) } else { None }
                },
            )
            .collect()
    }

    fn synthetic(
        attached_to: u32,
        text: &'static str,
        position: CommentPosition,
    ) -> SyntheticComment<'static> {
        SyntheticComment { attached_to, text, position }
    }

    #[test]
    fn sourced_leading_taken_once() {
        let allocator = Allocator::new();
        let source = "\
-- leading
local x = 1";
        let comments = [make_comment(0, 10, CommentKind::Line)];
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        let mut f = Formatter::new(&allocator, LuaVersion::Lua54, FormatOptions::default(), store);

        assert!(!f.write_stmt_leading_comments(11));
        assert_eq!(texts(&f), ["-- leading"]);
        f.write_stmt_leading_comments(11);
        assert_eq!(texts(&f), ["-- leading"]);
    }

    #[test]
    fn ignore_directive_detected() {
        let allocator = Allocator::new();
        let source = "\
-- luck: ignore
local x = 1";
        let comments = luck_lexer::lex(source, LuaVersion::Lua54).comments;
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        let mut f = Formatter::new(&allocator, LuaVersion::Lua54, FormatOptions::default(), store);
        assert!(f.write_stmt_leading_comments(16));
    }

    #[test]
    fn line_breaks_in_a_comment_print_as_lf() {
        let allocator = Allocator::new();
        let source = "--[[a\r\nb]]";
        let comments = [make_comment(0, 10, CommentKind::MultiLineBlock)];
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        let mut f = Formatter::new(&allocator, LuaVersion::Lua54, FormatOptions::default(), store);
        f.write_leading_comments(10);
        assert_eq!(texts(&f), ["--[[a\nb]]"]);
    }

    #[test]
    fn a_comment_starts_its_line_after_only_whitespace() {
        for (source, offset, version, expected) in [
            ("--", 0, LuaVersion::Lua54, true),
            ("x\n \t--", 4, LuaVersion::Lua54, true),
            ("x --", 2, LuaVersion::Lua54, false),
            ("x\r--", 2, LuaVersion::Lua54, true),
            // Luau reads a lone CR as a space.
            ("x\r--", 2, LuaVersion::Luau, false),
            ("x\r\n--", 3, LuaVersion::Luau, true),
        ] {
            assert_eq!(starts_line(source, offset, version), expected, "{source:?} in {version:?}");
        }
    }

    #[test]
    fn call_arguments_start_at_a_paren_a_table_or_a_string() {
        let source = "f --[[a]]\n-- b\n  (x)";
        let comments = [
            make_comment(2, 9, CommentKind::SingleLineBlock),
            make_comment(10, 14, CommentKind::Line),
        ];
        let Comments { store: Store::Sourced(sourced), .. } =
            Comments::from_source(&comments, source, LuaVersion::Lua54)
        else {
            panic!("a parsed store");
        };
        assert_eq!(sourced.next_token(1), b"(x)");
        for (token, expected) in [
            ("(x)", true),
            ("{}", true),
            ("'x'", true),
            ("\"x\"", true),
            ("[[x]]", true),
            ("[=[x]=]", true),
            ("[x]", false),
            (".x", false),
            (", x", false),
            ("", false),
        ] {
            assert_eq!(starts_call_arguments(token.as_bytes()), expected, "{token:?}");
        }
    }

    #[test]
    fn a_closer_is_found_past_the_comments_that_hold_one() {
        let source = "a --[[)]] -- )\n)";
        let comments = [
            make_comment(2, 9, CommentKind::SingleLineBlock),
            make_comment(10, 14, CommentKind::Line),
        ];
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        assert_eq!(store.closer_offset(1, b')'), 15);
        assert_eq!(Comments::none().closer_offset(1, b')'), 1);
    }

    #[test]
    fn a_line_ending_comment_outside_every_item_is_found() {
        let source = "(a --[[x]]\n, b --[[z]]) --[[y]]";
        let comments = [
            make_comment(3, 10, CommentKind::SingleLineBlock),
            Comment {
                is_followed_by_newline: false,
                ..make_comment(15, 22, CommentKind::SingleLineBlock)
            },
            make_comment(24, 31, CommentKind::SingleLineBlock),
        ];
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        let call = Span::new(0, 23);
        let has_outside =
            |items: &[Span]| store.has_line_ending_comment_outside(call, items.iter().copied());
        assert!(has_outside(&[Span::new(1, 2), Span::new(13, 14)]));
        assert!(!has_outside(&[Span::new(1, 10)]), "inside an item");
        assert!(!store.has_line_ending_comment_outside(Span::new(11, 23), iter::empty()));
    }

    #[test]
    fn synthetic_comments_attach_by_anchor() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([
            synthetic(5, "upvalue u0", CommentPosition::Leading),
            synthetic(5, "-- explicit marker", CommentPosition::Trailing),
        ]);
        let mut f = Formatter::new(&allocator, LuaVersion::Lua54, FormatOptions::default(), store);

        assert!(!f.write_stmt_leading_comments(5));
        assert_eq!(texts(&f), ["-- upvalue u0"], "marker prefix added to bare text");

        assert!(!f.write_stmt_trailing_comments(5, 0));
        let is_suffix = |element: &FormatElement<'_>| {
            matches!(element, FormatElement::LineSuffix("-- explicit marker", _, false))
        };
        assert!(f.elements().iter().any(is_suffix));
    }

    #[test]
    fn synthetic_comments_stay_off_the_nodes_inside_a_statement() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([synthetic(5, "kept", CommentPosition::Leading)]);
        let mut f = Formatter::new(&allocator, LuaVersion::Lua54, FormatOptions::default(), store);
        f.node(|| Span::new(5, 5), |_| {});
        assert_eq!(texts(&f), Vec::<&str>::new());
        f.write_stmt_leading_comments(5);
        assert_eq!(texts(&f), ["-- kept"]);
    }

    #[test]
    fn synthetic_dangling_drained_by_anchor() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([synthetic(9, "unreachable", CommentPosition::Leading)]);
        assert!(store.has_dangling_comments(9));
        assert!(!store.has_dangling_comments(8));

        let mut f = Formatter::new(&allocator, LuaVersion::Lua54, FormatOptions::default(), store);
        f.write_dangling_comments(9);
        assert_eq!(texts(&f), ["-- unreachable"]);
        assert!(!f.comments().has_dangling_comments(9));
    }

    #[test]
    fn synthetic_blank_before_recorded() {
        let comments = Comments::synthetic([]).with_blank_before([3, 7]);
        assert!(comments.has_synthetic_blank_before(3));
        assert!(comments.has_synthetic_blank_before(7));
        assert!(!comments.has_synthetic_blank_before(5));

        // Upgrades an empty store so `Comments::none()` users can opt in too.
        let from_none = Comments::none().with_blank_before([2]);
        assert!(from_none.has_synthetic_blank_before(2));
    }

    #[test]
    fn synthetic_remaining_flushes_in_anchor_order() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([
            synthetic(20, "second", CommentPosition::Leading),
            synthetic(10, "first", CommentPosition::Leading),
        ]);
        let mut f = Formatter::new(&allocator, LuaVersion::Lua54, FormatOptions::default(), store);
        f.write_remaining_comments(false);
        assert_eq!(texts(&f), ["-- first", "-- second"]);
    }

    #[test]
    fn format_off_disables_formatting_until_format_on() {
        let source = "\
-- luck: format off
local x=1
-- luck: format on
local y = 2";
        let source_comments = luck_lexer::lex(source, LuaVersion::Lua54).comments;
        let comments = Comments::from_source(&source_comments, source, LuaVersion::Lua54);
        assert!(comments.overlaps_disabled_region(Span::new(25, 26)));
        assert!(!comments.overlaps_disabled_region(Span::new(55, 56)));
    }
}
