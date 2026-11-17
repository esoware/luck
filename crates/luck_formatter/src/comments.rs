//! Parsed source carries comments as a sorted `[Comment]` keyed by byte
//! offset (`attached_to`); a synthetic AST carries `SyntheticComment`s keyed
//! by their anchor node's span start.
//!
//! A parsed comment's text is sliced from the source as it's written, and
//! only one whose line breaks need rewriting is copied, to the arena. The
//! sourced store is a cursor over the sorted array. The synthetic store is a
//! map keyed by anchor: synthesis order needn't match document order, and a
//! synthesizer may comment every statement. It also holds the anchors that
//! want a blank line before them ([`Comments::with_blank_before`]), since
//! there's no source to find blank lines in.

use alloc::collections::BTreeMap;
use core::iter::once;
use core::mem::take;
use core::ops::Range;

use luck_allocator::Allocator;
use luck_ast::SyntheticComment;
use luck_token::{
    Comment, CommentKind, CommentPosition, LuaVersion, Span, has_line_break, index_to_offset,
    is_lua_whitespace, line_breaks, offset_to_index, strip_line_break,
};
use rustc_hash::FxHashSet;

use crate::directive::{Directive, format_off_regions};
use crate::format_block::{has_blank_line, has_leading_edge_blank};
use crate::ir::Formatter;

#[derive(Debug)]
struct Sourced<'ast> {
    comments: &'ast [Comment],
    /// Kept for newline-gap checks and verbatim statement emission.
    source: &'ast str,
    version: LuaVersion,
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
        comments: BTreeMap<u32, Vec<SyntheticComment>>,
        blank_before_anchors: FxHashSet<u32>,
    },
    Empty,
}

/// The comments [`format_block`](crate::format_block()) places around a tree.
#[derive(Debug)]
pub struct Comments<'ast> {
    store: Store<'ast>,
}

/// Only the parsed path's cursor rewinds. The one speculative region, the
/// verbatim fallback, needs source, and a synthetic comment is taken once.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CommentsCheckpoint {
    printed_count: usize,
}

impl<'ast> Comments<'ast> {
    #[must_use]
    pub const fn none() -> Self {
        Self { store: Store::Empty }
    }

    pub(crate) fn from_source(
        comments: &'ast [Comment],
        source: &'ast str,
        version: LuaVersion,
    ) -> Self {
        let source_end = index_to_offset(source.len());
        let disabled_ranges = format_off_regions(comments, source)
            .into_iter()
            .map(|(off, on)| {
                let start = own_line_start(source, off.start)
                    .unwrap_or_else(|| after_line_break(source, off.end, version));
                start..on.map_or(source_end, |close| close.end)
            })
            .collect();
        let sourced = Sourced { comments, source, version, printed_count: 0, disabled_ranges };
        Self { store: Store::Sourced(sourced) }
    }

    #[must_use]
    pub fn synthetic<Synthetic>(comments: Synthetic) -> Self
    where
        Synthetic: IntoIterator<Item = SyntheticComment>,
    {
        let mut comments_by_anchor: BTreeMap<u32, Vec<SyntheticComment>> = BTreeMap::new();
        for comment in comments {
            comments_by_anchor.entry(comment.attached_to).or_default().push(comment);
        }
        Self {
            store: Store::Synthetic {
                comments: comments_by_anchor,
                blank_before_anchors: FxHashSet::default(),
            },
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

    /// The source and its dialect, on the parsed path.
    pub(crate) const fn source(&self) -> Option<(&'ast str, LuaVersion)> {
        match &self.store {
            Store::Sourced(sourced) => Some((sourced.source, sourced.version)),
            Store::Synthetic { .. } | Store::Empty => None,
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

    /// Narrow this store to `selection`: comments before it count as printed,
    /// and no `format off` region reaches outside it.
    pub(crate) fn restrict_to(&mut self, selection: Span) {
        if let Store::Sourced(sourced) = &mut self.store {
            sourced.printed_count =
                sourced.comments.partition_point(|c| c.span.start < selection.start);
            for region in &mut sourced.disabled_ranges {
                region.start = region.start.max(selection.start);
                region.end = region.end.min(selection.end);
            }
            sourced.disabled_ranges.retain(|region| region.start < region.end);
        }
    }

    /// Span start of the next unprinted comment (parsed path only), which
    /// blank-line logic uses to bound its gap scans.
    pub(crate) fn peek_next_start(&self) -> Option<u32> {
        match &self.store {
            Store::Sourced(sourced) => sourced.pending().first().map(|c| c.span.start),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    pub(crate) fn last_printed_end(&self) -> Option<u32> {
        match &self.store {
            Store::Sourced(sourced) => sourced.printed_end(),
            Store::Synthetic { .. } | Store::Empty => None,
        }
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
    ///
    /// The cursor is sequential, so an unprinted comment before `start` stops
    /// the mark and the ones inside stay pending. They aren't lost: the
    /// enclosing statement finds them still pending and goes verbatim over its
    /// whole span.
    pub(crate) fn mark_printed_range(&mut self, start: u32, end: u32) {
        if let Store::Sourced(sourced) = &mut self.store {
            sourced.take_while(|comment| (start..end).contains(&comment.span.start));
        }
    }

    /// Whether a comment in the body of an `if` ending at `if_end` keeps it
    /// from collapsing onto one line around `body_stmt`.
    ///
    /// The run right after `condition_end` is before `then`, so it stays out
    /// of the body. Of the rest, only a single-line block comment inside a
    /// `body_stmt` written on one line can stay put. Any other would have to
    /// move past `end`, break the line, or reindent on the next format.
    pub(crate) fn blocks_if_collapse(
        &self,
        condition_end: u32,
        body_stmt: Span,
        if_end: u32,
    ) -> bool {
        match &self.store {
            Store::Sourced(Sourced { comments, printed_count, source, version, .. }) => {
                // Only a comment inside the body needs this, and every `if` a
                // nest of them holds would scan the same text again.
                let mut is_body_one_line = None;
                let mut gap_start = condition_end;
                comments
                    .get(*printed_count..)
                    .unwrap_or_default()
                    .iter()
                    .skip_while(|c| c.span.start < condition_end)
                    .skip_while(|comment| {
                        let is_header = comment.kind == CommentKind::SingleLineBlock
                            && is_code_free(Span::new(gap_start, comment.span.start).text(source));
                        if is_header {
                            gap_start = comment.span.end;
                        }
                        is_header
                    })
                    .take_while(|c| c.span.start < if_end)
                    .any(|comment| {
                        comment.kind != CommentKind::SingleLineBlock
                            || !(body_stmt.start..body_stmt.end).contains(&comment.span.start)
                            || !*is_body_one_line.get_or_insert_with(|| {
                                !has_line_break(body_stmt.text(source), *version)
                            })
                    })
            }
            Store::Synthetic { comments, .. } => comments.contains_key(&body_stmt.start),
            Store::Empty => false,
        }
    }

    /// Whether a list-shaped construct covering `span` should claim its
    /// comments.
    ///
    /// It should when there's at least one and none falls inside an item, so
    /// a one-item-per-line layout can host them all. A comment inside an item
    /// belongs to that item's emitter or, failing that, to the
    /// statement-level verbatim fallback.
    pub(crate) fn has_claimable_comments(
        &self,
        span: Span,
        mut item_spans: impl Iterator<Item = Span>,
    ) -> bool {
        // A comment pending from before the construct blocks the cursor, so
        // the ones inside can't print either.
        if self.peek_next_start().is_none_or(|start| start < span.start) {
            return false;
        }
        !self.overlaps_disabled_region(span)
            && self.has_unhandled_in(span.start, span.end)
            && item_spans.all(|item| !self.has_unhandled_in(item.start, item.end))
    }

    pub(crate) fn has_unhandled_in(&self, start: u32, end: u32) -> bool {
        let Store::Sourced(sourced) = &self.store else {
            return false;
        };
        let pending = sourced.pending();
        // Sorted by position, so the first one at or past `start` is found
        // without walking the comments before it.
        let first_index = pending.partition_point(|c| c.span.start < start);
        pending.get(first_index).is_some_and(|c| c.span.start < end)
    }

    pub(crate) const fn checkpoint(&self) -> CommentsCheckpoint {
        CommentsCheckpoint {
            printed_count: match &self.store {
                Store::Sourced(sourced) => sourced.printed_count,
                Store::Synthetic { .. } | Store::Empty => 0,
            },
        }
    }

    pub(crate) const fn restore(&mut self, checkpoint: CommentsCheckpoint) {
        if let Store::Sourced(sourced) = &mut self.store {
            sourced.printed_count = checkpoint.printed_count;
        }
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

    /// Where the next `format off` region starts, past which no block-level
    /// writer may take a comment, since the region's own bytes hold it.
    fn disabled_region_limit(&self) -> u32 {
        let position = self.printed_end().unwrap_or(0);
        region_ending_after(&self.disabled_ranges, position).map_or(u32::MAX, |region| region.start)
    }

    /// Takes the pending comments while `should_take` holds.
    fn take_while(&mut self, mut should_take: impl FnMut(&Comment) -> bool) -> &'ast [Comment] {
        let pending = self.pending();
        let n = pending.iter().take_while(|c| should_take(c)).count();
        self.printed_count = self.printed_count.saturating_add(n);
        pending.get(..n).unwrap_or_default()
    }

    /// Takes the first `n` pending comments.
    fn take(&mut self, n: usize) -> &'ast [Comment] {
        let pending = self.pending();
        self.printed_count = self.printed_count.saturating_add(n);
        pending.get(..n).unwrap_or_default()
    }
}

/// The text a comment prints as, with its line breaks written as LF.
fn comment_text<'ast>(
    comment: &Comment,
    source: &'ast str,
    version: LuaVersion,
    allocator: &'ast Allocator,
) -> &'ast str {
    let text = comment.span.text(source);
    if comment.kind == CommentKind::Shebang {
        // Loaders skip this line up to LF, so a CR in it is text, but a
        // trailing one would pair with the line break written after it.
        return text.trim_end_matches('\r');
    }
    if !text.contains('\r') {
        return text;
    }
    let mut normalized = String::with_capacity(text.len());
    let mut cursor = 0;
    for line_break in line_breaks(text, version) {
        normalized.push_str(text.get(cursor..line_break.start).unwrap_or_default());
        normalized.push('\n');
        cursor = line_break.end;
    }
    normalized.push_str(text.get(cursor..).unwrap_or_default());
    allocator.alloc_str(&normalized)
}

impl<'ast> Formatter<'ast> {
    fn sourced_text(&self, comment: &Comment) -> &'ast str {
        self.comments().source().map_or("", |(source, version)| {
            comment_text(comment, source, version, self.allocator())
        })
    }

    /// Writes `comments` a line each, where `following` is the start of
    /// whatever comes after the last one, ending each with the line break it
    /// keeps. Returns true when one of them is a `luck: ignore` directive.
    fn write_comment_lines_from(&mut self, comments: &[Comment], following: u32) -> bool {
        let Some((source, version)) = self.comments().source() else {
            return false;
        };
        let mut has_ignore = false;
        let followers = comments.iter().skip(1).map(|c| c.span.start).chain(once(following));
        for (comment, next_start) in comments.iter().zip(followers) {
            let text = self.sourced_text(comment);
            has_ignore |= Directive::parse(text) == Some(Directive::Ignore);
            self.text(text);
            self.line_break(has_blank_line(source, comment.span.end, next_start, version));
        }
        has_ignore
    }

    /// Writes `comments` a line each with the line breaks between them, as
    /// [`Formatter::write_comment_lines_from`] does, but none after the last.
    fn write_comment_run_from(&mut self, comments: &[Comment]) {
        let Some((source, version)) = self.comments().source() else {
            return;
        };
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
        filter: impl Fn(&SyntheticComment) -> bool,
    ) -> Vec<&'ast str> {
        let allocator = self.allocator();
        let Store::Synthetic { comments, .. } = &mut self.comments_mut().store else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        if let Some(anchored_comments) = comments.get_mut(&anchor) {
            anchored_comments.retain(|comment| {
                if filter(comment) {
                    synthetic_comment_lines(&comment.text, allocator, &mut taken);
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
        let taken = sourced.take_while(|c| c.kind == CommentKind::Shebang);
        for comment in taken {
            let text = self.sourced_text(comment);
            self.text(text);
            self.hard_line();
        }
    }

    /// Returns true when one of them is a `luck: ignore` directive.
    pub(crate) fn write_leading_comments(&mut self, anchor: u32) -> bool {
        if let Some(sourced) = self.comments_mut().sourced_mut() {
            let limit = sourced.disabled_region_limit();
            let taken = sourced.take_while(|comment| {
                comment.span.start < limit
                    && comment.attached_to == anchor
                    && comment.position == CommentPosition::Leading
            });
            return self.write_comment_lines_from(taken, anchor);
        }
        let lines =
            self.take_synthetic(anchor, |comment| comment.position == CommentPosition::Leading);
        let mut has_ignore = false;
        for text in lines {
            has_ignore |= Directive::parse(text) == Some(Directive::Ignore);
            self.text(text);
            self.hard_line();
        }
        has_ignore
    }

    /// Write the comments stalled before the statement starting at `anchor`,
    /// each on its own line.
    ///
    /// These are attached to a token no emitter asked for, such as the `then`
    /// of an enclosing `if` or a `;` the block loop skips.
    ///
    /// Left pending, they'd block the cursor for every later comment. The
    /// statement's own leading comments end the run, since
    /// [`Formatter::write_leading_comments`] owns those.
    ///
    /// The run moves only if nothing but whitespace and `;` separates its
    /// comments from each other and from `anchor`, because crossing a token
    /// would relocate a comment. Otherwise it stays pending and the statement
    /// holding it falls back to verbatim.
    ///
    /// Returns true when one of them is a `luck: ignore` directive, as one
    /// parted from its statement by a `;` still applies to it.
    pub(crate) fn write_stalled_comments(&mut self, anchor: u32) -> bool {
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return false;
        };
        let limit = anchor.min(sourced.disabled_region_limit());
        let pending = sourced.pending();
        let candidate_count = pending
            .iter()
            .take_while(|comment| {
                comment.span.start < limit
                    && !(comment.attached_to == anchor
                        && comment.position == CommentPosition::Leading)
            })
            .count();
        let (candidates, after_candidates) = pending.split_at(candidate_count);
        // The run has to reach the statement's own leading comment when
        // `take_while` stopped on one, since that prints next, and the
        // anchor otherwise.
        let last_follower = after_candidates.first().map_or(limit, |c| c.span.start).min(limit);
        if !is_movable_run(candidates, sourced.source, last_follower) {
            return false;
        }
        let taken = sourced.take(candidate_count);
        self.write_comment_lines_from(taken, last_follower)
    }

    /// Returns true when one of them is a block comment spanning lines, which
    /// ends its line somewhere other than where the statement's does.
    pub(crate) fn write_trailing_comments(&mut self, anchor: u32, stmt_end: u32) -> bool {
        if let Some(sourced) = self.comments_mut().sourced_mut() {
            let (source, version) = (sourced.source, sourced.version);
            let mut gap_start = stmt_end;
            let taken = sourced.take_while(|comment| {
                if comment.position != CommentPosition::Trailing || gap_start > comment.span.start {
                    return false;
                }
                // A newline puts the comment on its own line, and a token in
                // between means it trails something else.
                let gap = Span::new(gap_start, comment.span.start).text(source);
                gap_start = comment.span.end;
                !has_line_break(gap, version) && is_code_free(gap)
            });
            let spans_lines =
                taken.iter().any(|comment| comment.kind == CommentKind::MultiLineBlock);
            for comment in taken {
                let text = self.sourced_text(comment);
                self.line_suffix(text);
            }
            return spans_lines;
        }
        let texts =
            self.take_synthetic(anchor, |comment| comment.position == CommentPosition::Trailing);
        for text in texts {
            self.line_suffix(text);
        }
        false
    }

    /// `start` is the list's own start.
    ///
    /// A pending comment before it was written outside the list, and taking
    /// it would drag it across the opening delimiter, so the cursor stops
    /// there and the statement-level fallback takes the whole run.
    ///
    /// Parsed input only: a synthetic comment has no offset to compare, and
    /// [`Formatter::write_leading_comments`] already places it by anchor.
    ///
    /// `can_follow_blank` says whether a line here keeps a blank line the
    /// source has above it, and is set once a line is written.
    fn write_own_line_comments_in(
        &mut self,
        start: u32,
        limit: u32,
        kind: ListKind,
        can_follow_blank: &mut bool,
    ) {
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return;
        };
        let (source, version) = (sourced.source, sourced.version);
        let taken = sourced.take_while(|c| (start..limit).contains(&c.span.start));
        for comment in taken {
            let has_blank_above = has_leading_edge_blank(source, comment.span.start, version);
            self.line_break(has_blank_above && *can_follow_blank);
            *can_follow_blank = kind == ListKind::TableFields;
            let text = self.sourced_text(comment);
            self.text(text);
        }
    }

    /// Only whitespace may come before each one, so every comment stays
    /// between the same two tokens.
    ///
    /// A line comment would swallow the rest of the line and a multi-line
    /// block comment would break it, so either one ends the run and is left to
    /// the statement-level verbatim fallback.
    pub(crate) fn write_inline_comments_after(&mut self, offset: u32) {
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return;
        };
        let source = sourced.source;
        let mut gap_start = offset;
        let taken = sourced.take_while(|comment| {
            if comment.span.start < gap_start
                || comment.kind != CommentKind::SingleLineBlock
                || !is_code_free(Span::new(gap_start, comment.span.start).text(source))
            {
                return false;
            }
            gap_start = comment.span.end;
            true
        });
        for comment in taken {
            self.space();
            let text = self.sourced_text(comment);
            self.text(text);
        }
    }

    /// The run ends at `limit`, the next item the caller writes.
    ///
    /// It also ends at a newline, since
    /// [`Formatter::write_own_line_comments_in`] takes the comments after
    /// one, and at a comment pending from before `offset`, which stalled
    /// outside this list and isn't the list's to place.
    fn write_same_line_comments_after(&mut self, offset: u32, limit: u32) {
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return;
        };
        let (source, version) = (sourced.source, sourced.version);
        let mut gap_start = offset;
        let taken = sourced.take_while(|comment| {
            if comment.span.start < gap_start || comment.span.start >= limit {
                return false;
            }
            let gap = Span::new(gap_start, comment.span.start).text(source);
            if has_line_break(gap, version) {
                return false;
            }
            gap_start = comment.span.end;
            true
        });
        for comment in taken {
            let text = self.sourced_text(comment);
            self.line_suffix(text);
        }
    }

    /// A packed or flat layout would push a line-suffix comment past the
    /// closing delimiter and out of the list.
    ///
    /// `span` covers the delimiters. [`Comments::has_claimable_comments`]
    /// decides whether a list may take this path, and guarantees that every
    /// comment here sits inside the delimiters and outside every item.
    pub(crate) fn write_commented_list<T>(
        &mut self,
        delimiters: (&'static str, &'static str),
        span: Span,
        items: &[T],
        kind: ListKind,
        span_of: fn(&T) -> Span,
        write_item: fn(&mut Self, &T),
    ) {
        let (open, close) = delimiters;
        self.token(open);
        self.indent(|f| {
            // The line after the opener never follows a blank one.
            let mut can_follow_blank = false;
            let mut remaining = items;
            while let [item, rest @ ..] = remaining {
                let item_span = span_of(item);
                let item_start = item_span.start;
                f.write_own_line_comments_in(span.start, item_start, kind, &mut can_follow_blank);
                let has_blank_above = can_follow_blank && f.has_blank_line_above(item_start);
                f.line_break(has_blank_above);
                can_follow_blank = kind == ListKind::TableFields;
                write_item(f, item);
                if kind == ListKind::TableFields || !rest.is_empty() {
                    f.token(",");
                }
                let next_start = rest.first().map_or(span.end, |next| span_of(next).start);
                f.write_same_line_comments_after(item_span.end, next_start);
                remaining = rest;
            }
            f.write_own_line_comments_in(span.start, span.end, kind, &mut can_follow_blank);
        });
        self.hard_line();
        self.token(close);
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
            let run = dangling_run(sourced.pending(), sourced.source, block_start, limit);
            let taken = sourced.take(run.len());
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
        let Some(sourced) = self.comments_mut().sourced_mut() else {
            return;
        };
        let (source, version) = (sourced.source, sourced.version);
        let limit = sourced.disabled_region_limit();
        let gap_start = sourced.printed_end().map_or(last_end, |end| end.max(last_end));
        let run = dangling_run(sourced.pending(), source, gap_start, limit);
        let taken = sourced.take(run.len());
        let Some(first) = taken.first() else {
            return;
        };
        self.line_break(has_blank_line(source, gap_start, first.span.start, version));
        self.write_comment_run_from(taken);
    }

    pub(crate) fn write_remaining_comments(&mut self, has_stmts_before: bool) {
        let allocator = self.allocator();
        let texts: Vec<&'ast str> = match &mut self.comments_mut().store {
            Store::Sourced(sourced) => {
                let (source, version) = (sourced.source, sourced.version);
                let taken = sourced.take(sourced.pending().len());
                taken.iter().map(|c| comment_text(c, source, version, allocator)).collect()
            }
            Store::Synthetic { comments, .. } => {
                let mut lines = Vec::new();
                for comment in take(comments).into_values().flatten() {
                    synthetic_comment_lines(&comment.text, allocator, &mut lines);
                }
                lines
            }
            Store::Empty => Vec::new(),
        };
        for (i, text) in texts.into_iter().enumerate() {
            if i > 0 || has_stmts_before {
                self.hard_line();
            }
            self.text(text);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListKind {
    /// A trailing comma is the expanded-layout convention, and a blank line
    /// above a field or a comment is kept.
    TableFields,
    /// No dialect's grammar allows a trailing comma.
    CallArgs,
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
/// from `start`, so an unmovable run, like a comment in an empty function's
/// parameter list that would have to cross the `)`, stays pending and the
/// statement holding it falls back to verbatim. The run stops at `limit`.
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
    text: &str,
    allocator: &'ast Allocator,
    lines: &mut Vec<&'ast str>,
) {
    let is_long_comment = text
        .trim_start()
        .strip_prefix("--[")
        .is_some_and(|rest| rest.trim_start_matches('=').starts_with('['));
    if is_long_comment {
        lines.push(allocator.alloc_str(text));
        return;
    }
    for line in text.lines().flat_map(|line| line.split('\r')) {
        lines.push(if line.trim_start().starts_with("--") {
            allocator.alloc_str(line)
        } else if line.is_empty() {
            "--"
        } else {
            allocator.alloc_concat_strs_array(["-- ", line])
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FormatOptions;
    use crate::ir::FormatElement;

    fn make_comment(
        start: u32,
        end: u32,
        attached_to: u32,
        kind: CommentKind,
        position: CommentPosition,
    ) -> Comment {
        Comment {
            span: Span::new(start, end),
            attached_to,
            kind,
            position,
            is_preceded_by_newline: false,
            is_followed_by_newline: false,
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

    fn synthetic(attached_to: u32, text: &str, position: CommentPosition) -> SyntheticComment {
        SyntheticComment { attached_to, text: text.into(), position }
    }

    #[test]
    fn sourced_leading_taken_once() {
        let allocator = Allocator::new();
        let source = "\
-- leading
local x = 1";
        let comments = [make_comment(0, 10, 11, CommentKind::Line, CommentPosition::Leading)];
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        let mut f = Formatter::new(&allocator, FormatOptions::default(), store);

        assert!(!f.write_leading_comments(11));
        assert_eq!(texts(&f), ["-- leading"]);
        f.write_leading_comments(11);
        assert_eq!(texts(&f), ["-- leading"]);
    }

    #[test]
    fn ignore_directive_detected() {
        let allocator = Allocator::new();
        let source = "\
-- luck: ignore
local x = 1";
        let comments = [make_comment(0, 15, 16, CommentKind::Line, CommentPosition::Leading)];
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        let mut f = Formatter::new(&allocator, FormatOptions::default(), store);
        assert!(f.write_leading_comments(16));
    }

    #[test]
    fn line_breaks_in_a_comment_print_as_lf() {
        let allocator = Allocator::new();
        let source = "--[[a\r\nb]]";
        let comments =
            [make_comment(0, 10, 10, CommentKind::MultiLineBlock, CommentPosition::Leading)];
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        let mut f = Formatter::new(&allocator, FormatOptions::default(), store);
        f.write_remaining_comments(false);
        assert_eq!(texts(&f), ["--[[a\nb]]"]);
    }

    #[test]
    fn synthetic_comments_attach_by_anchor() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([
            synthetic(5, "upvalue u0", CommentPosition::Leading),
            synthetic(5, "-- explicit marker", CommentPosition::Trailing),
        ]);
        let mut f = Formatter::new(&allocator, FormatOptions::default(), store);

        assert!(!f.write_leading_comments(5));
        assert_eq!(texts(&f), ["-- upvalue u0"], "marker prefix added to bare text");

        assert!(!f.write_trailing_comments(5, 0));
        let is_suffix = |element: &FormatElement<'_>| {
            matches!(element, FormatElement::LineSuffix("-- explicit marker", _))
        };
        assert!(f.elements().iter().any(is_suffix));
    }

    #[test]
    fn synthetic_dangling_drained_by_anchor() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([synthetic(9, "unreachable", CommentPosition::Leading)]);
        assert!(store.has_dangling_comments(9));
        assert!(!store.has_dangling_comments(8));

        let mut f = Formatter::new(&allocator, FormatOptions::default(), store);
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
        let mut f = Formatter::new(&allocator, FormatOptions::default(), store);
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
        let source_comments = [
            make_comment(0, 19, 20, CommentKind::Line, CommentPosition::Leading),
            make_comment(30, 49, 50, CommentKind::Line, CommentPosition::Leading),
        ];
        let comments = Comments::from_source(&source_comments, source, LuaVersion::Lua54);
        assert!(comments.overlaps_disabled_region(Span::new(25, 26)));
        assert!(!comments.overlaps_disabled_region(Span::new(55, 56)));
    }
}
