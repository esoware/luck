//! Parsed source carries comments as a sorted `Vec<Comment>` keyed by byte
//! offset (`attached_to`); a synthetic AST carries `SyntheticComment`s keyed
//! by their anchor node's span start.
//!
//! Both hold owned text, so printing a
//! comment never slices source.
//!
//! The sourced store is a cursor over the sorted array. The synthetic store
//! is a map keyed by anchor: synthesis order needn't match document order,
//! and a synthesizer may comment every statement. It also holds the anchors
//! that want a blank line before them ([`Comments::with_blank_before`]),
//! since there's no source to find blank lines in.

use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use core::iter::once;
use core::mem::take;
use core::ops::Range;

use luck_ast::SyntheticComment;
use luck_token::{
    Comment, CommentKind, CommentPosition, CompactString, LuaVersion, Span, has_line_break,
    index_to_offset, is_lua_whitespace, line_breaks, offset_to_index, strip_line_break,
};
use rustc_hash::FxHashSet;

use crate::directive::{Directive, format_off_regions};
use crate::format_block::{has_blank_line, has_blank_line_above, has_leading_edge_blank};
use crate::ir::{Format, Formatter, empty_line, format_with, hard_line, indent, space, token};

#[derive(Debug, Clone)]
struct SourcedComment {
    text: CompactString,
    attached_to: u32,
    span: Span,
    position: CommentPosition,
    kind: CommentKind,
}

#[derive(Debug)]
enum Store {
    Sourced {
        comments: Vec<SourcedComment>,
        /// Kept for newline-gap checks and verbatim statement emission.
        source: String,
        version: LuaVersion,
        printed_count: usize,
        /// The spans `-- luck: format off` / `on` regions copy as written,
        /// the directive comments and their own lines included. Sorted, and
        /// empty on almost every file.
        disabled_ranges: Vec<Range<u32>>,
    },
    Synthetic {
        /// Keyed by anchor and drained as each anchor is formatted. Whatever
        /// is left flushes at end of file in anchor order.
        comments: BTreeMap<u32, Vec<SyntheticComment>>,
        blank_before_anchors: FxHashSet<u32>,
    },
    Empty,
}

#[derive(Debug)]
pub struct Comments {
    store: Store,
}

/// Only the parsed path's cursor rewinds. The one speculative region, the
/// verbatim fallback, needs source, and a synthetic comment is taken once.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CommentsCheckpoint {
    printed_count: usize,
}

impl Comments {
    #[must_use]
    pub const fn none() -> Self {
        Self { store: Store::Empty }
    }

    #[must_use]
    pub fn from_source(comments: &[Comment], source: &str, version: LuaVersion) -> Self {
        let sourced_comments = comments
            .iter()
            .map(|comment| SourcedComment {
                text: CompactString::from(if comment.kind == CommentKind::Shebang {
                    // Loaders skip this line up to LF, so a CR in it is text, but a
                    // trailing one would pair with the line break written after it.
                    Cow::Borrowed(comment.span.text(source).trim_end_matches('\r'))
                } else {
                    unix_line_breaks(comment.span.text(source), version)
                }),
                attached_to: comment.attached_to,
                span: comment.span,
                position: comment.position,
                kind: comment.kind,
            })
            .collect();

        let source_end = index_to_offset(source.len());
        let disabled_ranges = format_off_regions(comments, source)
            .into_iter()
            .map(|(off, on)| {
                let start = own_line_start(source, off.start)
                    .unwrap_or_else(|| after_line_break(source, off.end, version));
                start..on.map_or(source_end, |close| close.end)
            })
            .collect();

        Self {
            store: Store::Sourced {
                comments: sourced_comments,
                source: source.to_owned(),
                version,
                printed_count: 0,
                disabled_ranges,
            },
        }
    }

    #[must_use]
    pub fn synthetic(comments: Vec<SyntheticComment>) -> Self {
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
    /// `anchors`. Synthetic input only; parsed input gets its blank lines
    /// from the source.
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
            Store::Sourced { .. } | Store::Empty => false,
        }
    }

    pub(crate) fn source_text(&self) -> Option<&str> {
        match &self.store {
            Store::Sourced { source, .. } => Some(source),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    /// The verbatim fallback re-lexes its statement with it to tell line
    /// breaks from newlines inside a long string or comment.
    pub(crate) const fn source_version(&self) -> Option<LuaVersion> {
        match &self.store {
            Store::Sourced { version, .. } => Some(*version),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    /// The first `format off` region not yet behind `position` or the last
    /// printed comment.
    pub(crate) fn next_disabled_region(&self, position: u32) -> Option<Range<u32>> {
        let Store::Sourced { disabled_ranges, .. } = &self.store else {
            return None;
        };
        let reached = self.last_printed_end().map_or(position, |end| end.max(position));
        region_ending_after(disabled_ranges, reached).cloned()
    }

    pub(crate) fn overlaps_disabled_region(&self, span: Span) -> bool {
        match &self.store {
            Store::Sourced { disabled_ranges, .. } => {
                region_ending_after(disabled_ranges, span.start)
                    .is_some_and(|region| region.start < span.end)
            }
            Store::Synthetic { .. } | Store::Empty => false,
        }
    }

    /// Whether `from..to` holds nothing but comments, whitespace and `;`.
    pub(crate) fn is_free_of_tokens(&self, from: u32, to: u32) -> bool {
        let Store::Sourced { comments, source, .. } = &self.store else {
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
        if let Store::Sourced { comments, printed_count, disabled_ranges, .. } = &mut self.store {
            *printed_count = comments.partition_point(|c| c.span.start < selection.start);
            for region in disabled_ranges.iter_mut() {
                region.start = region.start.max(selection.start);
                region.end = region.end.min(selection.end);
            }
            disabled_ranges.retain(|region| region.start < region.end);
        }
    }

    /// Span start of the next unprinted comment (parsed path only), which
    /// blank-line logic uses to bound its gap scans.
    pub(crate) fn peek_next_start(&self) -> Option<u32> {
        match &self.store {
            Store::Sourced { comments, printed_count, .. } => {
                comments.get(*printed_count).map(|c| c.span.start)
            }
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    pub(crate) fn last_printed_end(&self) -> Option<u32> {
        match &self.store {
            Store::Sourced { comments, printed_count, .. } => printed_end(comments, *printed_count),
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
            Store::Sourced { comments, printed_count, source, disabled_ranges, .. } => {
                let limit = disabled_region_limit(disabled_ranges, comments, *printed_count);
                let pending = pending_comments(comments, *printed_count);
                !dangling_run(pending, source, block_start, limit).is_empty()
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
        if let Store::Sourced { comments, printed_count, .. } = &mut self.store {
            take_pending_while(comments, printed_count, |comment| {
                (start..end).contains(&comment.span.start)
            });
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
            Store::Sourced { comments, printed_count, source, version, .. } => {
                let is_body_one_line = !has_line_break(body_stmt.text(source), *version);
                let mut gap_start = condition_end;
                pending_comments(comments, *printed_count)
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
                            || !is_body_one_line
                            || !(body_stmt.start..body_stmt.end).contains(&comment.span.start)
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
        let pending = match &self.store {
            Store::Sourced { comments, printed_count, .. } => {
                pending_comments(comments, *printed_count)
            }
            Store::Synthetic { .. } | Store::Empty => &[],
        };
        // Sorted by position, so the first one at or past `start` is found
        // without walking the comments before it.
        let first_index = pending.partition_point(|c| c.span.start < start);
        pending.get(first_index).is_some_and(|c| c.span.start < end)
    }

    pub(crate) const fn checkpoint(&self) -> CommentsCheckpoint {
        CommentsCheckpoint {
            printed_count: match &self.store {
                Store::Sourced { printed_count, .. } => *printed_count,
                Store::Synthetic { .. } | Store::Empty => 0,
            },
        }
    }

    pub(crate) const fn restore(&mut self, checkpoint: CommentsCheckpoint) {
        if let Store::Sourced { printed_count, .. } = &mut self.store {
            *printed_count = checkpoint.printed_count;
        }
    }

    /// Parsed input only: a synthetic comment has no offset or source to test.
    fn take_sourced_while(
        &mut self,
        mut should_take: impl FnMut(&SourcedComment, &str) -> bool,
    ) -> Vec<CompactString> {
        match &mut self.store {
            Store::Sourced { comments, printed_count, source, .. } => {
                comment_texts(take_pending_while(comments, printed_count, |comment| {
                    should_take(comment, source)
                }))
            }
            Store::Synthetic { .. } | Store::Empty => Vec::new(),
        }
    }
}

pub(crate) fn write_shebang(f: &mut Formatter) {
    let texts = f.comments_mut().take_sourced_while(|c, _| c.kind == CommentKind::Shebang);
    write_comment_lines(f, lines_without_blanks(texts));
}

/// Returns true when one of them is a `luck: ignore` directive.
pub(crate) fn write_leading_comments(f: &mut Formatter, anchor: u32) -> bool {
    let lines = match &mut f.comments_mut().store {
        Store::Sourced { comments, printed_count, source, version, disabled_ranges } => {
            let limit = disabled_region_limit(disabled_ranges, comments, *printed_count);
            let taken = take_pending_while(comments, printed_count, |comment| {
                comment.span.start < limit
                    && comment.attached_to == anchor
                    && comment.position == CommentPosition::Leading
            });
            comment_lines(taken, source, *version, anchor)
        }
        Store::Synthetic { comments, .. } => {
            lines_without_blanks(take_synthetic(comments, anchor, |comment| {
                comment.position == CommentPosition::Leading
            }))
        }
        Store::Empty => Vec::new(),
    };
    write_comment_lines(f, lines)
}

/// Write the comments stalled before the statement starting at `anchor`,
/// each on its own line.
///
/// These are attached to a token no emitter asked for, such as the `then` of
/// an enclosing `if` or a `;` the block loop skips.
///
/// Left pending, they'd block the cursor for every later comment. The statement's own leading
/// comments end the run, since [`write_leading_comments`] owns those.
///
/// The run moves only if nothing but whitespace and `;` separates its
/// comments from each other and from `anchor`, because crossing a token would
/// relocate a comment. Otherwise it stays pending and the statement holding
/// it falls back to verbatim.
///
/// Returns true when one of them is a `luck: ignore` directive, as one parted
/// from its statement by a `;` still applies to it.
pub(crate) fn write_stalled_comments(f: &mut Formatter, anchor: u32) -> bool {
    let lines = match &mut f.comments_mut().store {
        Store::Sourced { comments, printed_count, source, version, disabled_ranges } => {
            let limit =
                anchor.min(disabled_region_limit(disabled_ranges, comments, *printed_count));
            let pending = pending_comments(comments, *printed_count);
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
            if is_movable_run(candidates, source, last_follower) {
                *printed_count = printed_count.saturating_add(candidate_count);
                comment_lines(candidates, source, *version, last_follower)
            } else {
                Vec::new()
            }
        }
        Store::Synthetic { .. } | Store::Empty => Vec::new(),
    };
    write_comment_lines(f, lines)
}

pub(crate) fn write_trailing_comments(f: &mut Formatter, anchor: u32, stmt_end: u32) {
    let texts = match &mut f.comments_mut().store {
        Store::Sourced { comments, printed_count, source, version, .. } => {
            let mut gap_start = stmt_end;
            comment_texts(take_pending_while(comments, printed_count, |comment| {
                if comment.position != CommentPosition::Trailing || gap_start > comment.span.start {
                    return false;
                }
                // A newline puts the comment on its own line, and a token in
                // between means it trails something else.
                let gap = Span::new(gap_start, comment.span.start).text(source);
                gap_start = comment.span.end;
                !has_line_break(gap, *version) && is_code_free(gap)
            }))
        }
        Store::Synthetic { comments, .. } => take_synthetic(comments, anchor, |comment| {
            comment.position == CommentPosition::Trailing
        }),
        Store::Empty => Vec::new(),
    };
    for text in texts {
        f.write_line_suffix(text);
    }
}

/// `start` is the list's own start.
///
/// A pending comment before it was written outside the list, and taking it would drag it across the
/// opening delimiter, so the cursor stops there and the statement-level fallback takes the whole
/// run.
///
/// Parsed input only: a synthetic comment has no offset to compare, and
/// [`write_leading_comments`] already places it by anchor.
///
/// `can_follow_blank` says whether a line here keeps a blank line the source
/// has above it, and is set once a line is written.
fn write_own_line_comments_in(
    f: &mut Formatter,
    start: u32,
    limit: u32,
    kind: ListKind,
    can_follow_blank: &mut bool,
) {
    let version = f.comments().source_version();
    let mut blanks_above = Vec::new();
    let texts = f.comments_mut().take_sourced_while(|c, source| {
        let is_inside = (start..limit).contains(&c.span.start);
        if is_inside {
            blanks_above.push(version.is_some_and(|source_version| {
                has_leading_edge_blank(source, c.span.start, source_version)
            }));
        }
        is_inside
    });
    for (text, has_blank_above) in texts.into_iter().zip(blanks_above) {
        write_line_break(f, has_blank_above && *can_follow_blank);
        *can_follow_blank = kind == ListKind::TableFields;
        f.write_text(text);
    }
}

/// Only whitespace may come before each one, so every comment stays between
/// the same two tokens.
///
/// A line comment would swallow the rest of the line and
/// a multi-line block comment would break it, so either one ends the run and
/// is left to the statement-level verbatim fallback.
pub(crate) fn write_inline_comments_after(f: &mut Formatter, offset: u32) {
    let mut gap_start = offset;
    let texts = f.comments_mut().take_sourced_while(|comment, source| {
        if comment.span.start < gap_start
            || comment.kind != CommentKind::SingleLineBlock
            || !is_code_free(Span::new(gap_start, comment.span.start).text(source))
        {
            return false;
        }
        gap_start = comment.span.end;
        true
    });
    for text in texts {
        space().fmt(f);
        f.write_text(text);
    }
}

/// The run ends at `limit`, the next item the caller writes.
///
/// It also ends at a newline, since [`write_own_line_comments_in`] takes the comments after one,
/// and at a comment pending from before `offset`, which stalled outside this list and isn't the
/// list's to place.
fn write_same_line_comments_after(f: &mut Formatter, offset: u32, limit: u32) {
    let mut gap_start = offset;
    let Some(version) = f.comments().source_version() else {
        return;
    };
    let texts = f.comments_mut().take_sourced_while(|comment, source| {
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
    for text in texts {
        f.write_line_suffix(text);
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

/// A packed or flat layout would push a line-suffix comment past the closing
/// delimiter and out of the list.
///
/// `span` covers the delimiters. [`Comments::has_claimable_comments`] decides
/// whether a list may take this path, and guarantees that every comment here
/// sits inside the delimiters and outside every item.
pub(crate) fn write_commented_list<T>(
    f: &mut Formatter,
    delimiters: (&'static str, &'static str),
    span: Span,
    items: &[T],
    span_of: fn(&T) -> Span,
    kind: ListKind,
) where
    T: Format,
{
    let (open, close) = delimiters;
    token(open).fmt(f);
    indent(format_with(|indented| {
        // The line after the opener never follows a blank one.
        let mut can_follow_blank = false;
        let mut remaining = items;
        while let [item, rest @ ..] = remaining {
            let item_span = span_of(item);
            let item_start = item_span.start;
            write_own_line_comments_in(
                indented,
                span.start,
                item_start,
                kind,
                &mut can_follow_blank,
            );
            let has_blank_above = can_follow_blank && has_blank_line_above(indented, item_start);
            write_line_break(indented, has_blank_above);
            can_follow_blank = kind == ListKind::TableFields;
            item.fmt(indented);
            if kind == ListKind::TableFields || !rest.is_empty() {
                token(",").fmt(indented);
            }
            let next_start = rest.first().map_or(span.end, |next| span_of(next).start);
            write_same_line_comments_after(indented, item_span.end, next_start);
            remaining = rest;
        }
        write_own_line_comments_in(indented, span.start, span.end, kind, &mut can_follow_blank);
    }))
    .fmt(f);
    (hard_line(), token(close)).fmt(f);
}

/// Write the comments in a block with no statements, whose span starts at
/// `block_start`, one per line.
///
/// On the parsed path that's the [`dangling_run`] there; on the synthetic path,
/// every comment anchored at `block_start`. The caller writes the line breaks
/// around them.
pub(crate) fn write_dangling_comments(f: &mut Formatter, block_start: u32) {
    let lines = match &mut f.comments_mut().store {
        Store::Sourced { comments, printed_count, source, version, disabled_ranges } => {
            let limit = disabled_region_limit(disabled_ranges, comments, *printed_count);
            let run = dangling_run(
                pending_comments(comments, *printed_count),
                source,
                block_start,
                limit,
            );
            *printed_count = printed_count.saturating_add(run.len());
            comment_lines(run, source, *version, block_start)
        }
        Store::Synthetic { comments, .. } => {
            lines_without_blanks(take_synthetic(comments, block_start, |_| true))
        }
        Store::Empty => Vec::new(),
    };
    write_comment_run(f, lines);
}

/// Write the comments after a block's last statement, each on its own line
/// and keeping the blank lines around them.
///
/// That's the [`dangling_run`] after `last_end`, so a comment only moves past
/// whitespace and `;`. Parsed input only: a synthetic comment names its anchor.
pub(crate) fn write_tail_comments(f: &mut Formatter, last_end: u32) {
    let (lines, has_blank_before) = match &mut f.comments_mut().store {
        Store::Sourced { comments, printed_count, source, version, disabled_ranges } => {
            let limit = disabled_region_limit(disabled_ranges, comments, *printed_count);
            let gap_start =
                printed_end(comments, *printed_count).map_or(last_end, |end| end.max(last_end));
            let run =
                dangling_run(pending_comments(comments, *printed_count), source, gap_start, limit);
            *printed_count = printed_count.saturating_add(run.len());
            let has_blank_before = run.first().is_some_and(|comment| {
                has_blank_line(source, gap_start, comment.span.start, *version)
            });
            (comment_lines(run, source, *version, gap_start), has_blank_before)
        }
        Store::Synthetic { .. } | Store::Empty => return,
    };
    if !lines.is_empty() {
        write_line_break(f, has_blank_before);
        write_comment_run(f, lines);
    }
}

pub(crate) fn write_remaining_comments(f: &mut Formatter, has_stmts_before: bool) {
    let texts = match &mut f.comments_mut().store {
        Store::Sourced { comments, printed_count, .. } => {
            let taken = comment_texts(pending_comments(comments, *printed_count));
            *printed_count = comments.len();
            taken
        }
        Store::Synthetic { comments, .. } => take(comments)
            .into_values()
            .flatten()
            .flat_map(|c| synthetic_comment_lines(&c.text))
            .collect(),
        Store::Empty => Vec::new(),
    };
    for (i, text) in texts.into_iter().enumerate() {
        if i > 0 || has_stmts_before {
            hard_line().fmt(f);
        }
        f.write_text(text);
    }
}

struct CommentLine {
    text: CompactString,
    has_blank_after: bool,
}

/// `comments` as lines, where `following` is the start of whatever comes
/// after the last one.
fn comment_lines(
    comments: &[SourcedComment],
    source: &str,
    version: LuaVersion,
    following: u32,
) -> Vec<CommentLine> {
    let followers = comments.iter().skip(1).map(|c| c.span.start).chain(once(following));
    comments
        .iter()
        .zip(followers)
        .map(|(comment, next_start)| CommentLine {
            text: comment.text.clone(),
            has_blank_after: has_blank_line(source, comment.span.end, next_start, version),
        })
        .collect()
}

fn lines_without_blanks(texts: Vec<CompactString>) -> Vec<CommentLine> {
    texts.into_iter().map(|text| CommentLine { text, has_blank_after: false }).collect()
}

fn write_line_break(f: &mut Formatter, is_blank: bool) {
    if is_blank {
        empty_line().fmt(f);
    } else {
        hard_line().fmt(f);
    }
}

/// Returns true when one of them is a `luck: ignore` directive.
fn write_comment_lines(f: &mut Formatter, lines: Vec<CommentLine>) -> bool {
    let has_ignore =
        lines.iter().any(|line| Directive::parse(&line.text) == Some(Directive::Ignore));
    for line in lines {
        f.write_text(line.text);
        write_line_break(f, line.has_blank_after);
    }
    has_ignore
}

fn write_comment_run(f: &mut Formatter, lines: Vec<CommentLine>) {
    let mut previous_has_blank = None;
    for line in lines {
        if let Some(is_blank) = previous_has_blank {
            write_line_break(f, is_blank);
        }
        previous_has_blank = Some(line.has_blank_after);
        f.write_text(line.text);
    }
}

fn pending_comments(comments: &[SourcedComment], printed_count: usize) -> &[SourcedComment] {
    comments.get(printed_count..).unwrap_or_default()
}

fn printed_end(comments: &[SourcedComment], printed_count: usize) -> Option<u32> {
    let last_index = printed_count.checked_sub(1)?;
    comments.get(last_index).map(|c| c.span.end)
}

/// Where the next `format off` region starts, past which no block-level
/// writer may take a comment, since the region's own bytes hold it.
fn disabled_region_limit(
    disabled_ranges: &[Range<u32>],
    comments: &[SourcedComment],
    printed_count: usize,
) -> u32 {
    let position = printed_end(comments, printed_count).unwrap_or(0);
    region_ending_after(disabled_ranges, position).map_or(u32::MAX, |region| region.start)
}

/// The first of `regions` that ends past `position`. `format off` regions are
/// sorted and apart, so their ends are in order too.
pub(crate) fn region_ending_after(regions: &[Range<u32>], position: u32) -> Option<&Range<u32>> {
    regions.get(regions.partition_point(|region| region.end <= position))
}

fn take_pending_while<'comments, F>(
    comments: &'comments [SourcedComment],
    printed_count: &mut usize,
    mut should_take: F,
) -> &'comments [SourcedComment]
where
    F: FnMut(&SourcedComment) -> bool,
{
    let pending = pending_comments(comments, *printed_count);
    let n = pending.iter().take_while(|c| should_take(c)).count();
    *printed_count = printed_count.saturating_add(n);
    let (taken, _) = pending.split_at(n);
    taken
}

fn comment_texts(comments: &[SourcedComment]) -> Vec<CompactString> {
    comments.iter().map(|c| c.text.clone()).collect()
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
    pending: &'comments [SourcedComment],
    source: &str,
    start: u32,
    limit: u32,
) -> &'comments [SourcedComment] {
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
fn is_movable_run(run: &[SourcedComment], source: &str, run_end: u32) -> bool {
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

pub(crate) fn unix_line_breaks(text: &str, version: LuaVersion) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    let mut normalized = String::with_capacity(text.len());
    let mut cursor = 0;
    for line_break in line_breaks(text, version) {
        normalized.push_str(text.get(cursor..line_break.start).unwrap_or_default());
        normalized.push('\n');
        cursor = line_break.end;
    }
    normalized.push_str(text.get(cursor..).unwrap_or_default());
    Cow::Owned(normalized)
}

/// Synthetic text may leave out the `--` marker, so each line gets one when
/// missing. A long comment carries its line breaks as content and stays whole.
fn synthetic_comment_lines(text: &str) -> Vec<CompactString> {
    let is_long_comment = text
        .trim_start()
        .strip_prefix("--[")
        .is_some_and(|rest| rest.trim_start_matches('=').starts_with('['));
    if is_long_comment {
        return vec![CompactString::from(text)];
    }
    text.lines()
        .flat_map(|line| line.split('\r'))
        .map(|line| {
            if line.trim_start().starts_with("--") {
                CompactString::from(line)
            } else if line.is_empty() {
                CompactString::from("--")
            } else {
                let mut prefixed = CompactString::from("-- ");
                prefixed.push_str(line);
                prefixed
            }
        })
        .collect()
}

fn take_synthetic(
    comments: &mut BTreeMap<u32, Vec<SyntheticComment>>,
    anchor: u32,
    filter: impl Fn(&SyntheticComment) -> bool,
) -> Vec<CompactString> {
    let mut taken = Vec::new();
    if let Some(anchored_comments) = comments.get_mut(&anchor) {
        anchored_comments.retain(|comment| {
            if filter(comment) {
                taken.extend(synthetic_comment_lines(&comment.text));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FormatOptions;
    use crate::ir::{FormatElement, Tag};

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

    fn formatter_over(comments: Comments) -> Formatter {
        Formatter::new(FormatOptions::default(), comments)
    }

    #[test]
    fn sourced_leading_taken_once() {
        let source = "-- leading\nlocal x = 1";
        let comments = vec![make_comment(0, 10, 11, CommentKind::Line, CommentPosition::Leading)];
        let mut f = formatter_over(Comments::from_source(&comments, source, LuaVersion::Lua54));

        assert!(!write_leading_comments(&mut f, 11));
        let first_element_count = f.elements().len();
        assert!(first_element_count > 0);
        write_leading_comments(&mut f, 11);
        assert_eq!(f.elements().len(), first_element_count);
    }

    #[test]
    fn ignore_directive_detected() {
        let source = "-- luck: ignore\nlocal x = 1";
        let comments = vec![make_comment(0, 15, 16, CommentKind::Line, CommentPosition::Leading)];
        let mut f = formatter_over(Comments::from_source(&comments, source, LuaVersion::Lua54));
        assert!(write_leading_comments(&mut f, 16));
    }

    #[test]
    fn synthetic_comments_attach_by_anchor() {
        let synthetic = vec![
            SyntheticComment {
                attached_to: 5,
                text: "upvalue u0".into(),
                position: CommentPosition::Leading,
            },
            SyntheticComment {
                attached_to: 5,
                text: "-- explicit marker".into(),
                position: CommentPosition::Trailing,
            },
        ];
        let mut f = formatter_over(Comments::synthetic(synthetic));

        assert!(!write_leading_comments(&mut f, 5));
        let has_prefixed = f.elements().iter().any(
            |element| matches!(element, FormatElement::Text(text, _) if text == "-- upvalue u0"),
        );
        assert!(has_prefixed, "marker prefix added to bare text");

        write_trailing_comments(&mut f, 5, 0);
        let has_suffix = f
            .elements()
            .iter()
            .any(|element| matches!(element, FormatElement::Tag(Tag::StartLineSuffix)));
        assert!(has_suffix);
    }

    #[test]
    fn synthetic_dangling_drained_by_anchor() {
        let synthetic = vec![SyntheticComment {
            attached_to: 9,
            text: "unreachable".into(),
            position: CommentPosition::Leading,
        }];
        let comments = Comments::synthetic(synthetic);
        assert!(comments.has_dangling_comments(9));
        assert!(!comments.has_dangling_comments(8));

        let mut f = formatter_over(comments);
        write_dangling_comments(&mut f, 9);
        let is_emitted = f.elements().iter().any(
            |element| matches!(element, FormatElement::Text(text, _) if text == "-- unreachable"),
        );
        assert!(is_emitted);
        assert!(!f.comments().has_dangling_comments(9));
    }

    #[test]
    fn synthetic_blank_before_recorded() {
        let comments = Comments::synthetic(vec![]).with_blank_before([3, 7]);
        assert!(comments.has_synthetic_blank_before(3));
        assert!(comments.has_synthetic_blank_before(7));
        assert!(!comments.has_synthetic_blank_before(5));

        // Upgrades an empty store so `Comments::none()` users can opt in too.
        let from_none = Comments::none().with_blank_before([2]);
        assert!(from_none.has_synthetic_blank_before(2));
    }

    #[test]
    fn synthetic_remaining_flushes_in_anchor_order() {
        let synthetic = vec![
            SyntheticComment {
                attached_to: 20,
                text: "second".into(),
                position: CommentPosition::Leading,
            },
            SyntheticComment {
                attached_to: 10,
                text: "first".into(),
                position: CommentPosition::Leading,
            },
        ];
        let mut f = formatter_over(Comments::synthetic(synthetic));
        write_remaining_comments(&mut f, false);
        let texts: Vec<&str> = f
            .elements()
            .iter()
            .filter_map(|element| {
                if let FormatElement::Text(text, _) = element { Some(text.as_str()) } else { None }
            })
            .collect();
        assert_eq!(texts, vec!["-- first", "-- second"]);
    }

    #[test]
    fn format_off_disables_formatting_until_format_on() {
        let source = "-- luck: format off\nlocal x=1\n-- luck: format on\nlocal y = 2";
        let source_comments = vec![
            make_comment(0, 19, 20, CommentKind::Line, CommentPosition::Leading),
            make_comment(30, 49, 50, CommentKind::Line, CommentPosition::Leading),
        ];
        let comments = Comments::from_source(&source_comments, source, LuaVersion::Lua54);
        assert!(comments.overlaps_disabled_region(Span::new(25, 26)));
        assert!(!comments.overlaps_disabled_region(Span::new(55, 56)));
    }
}
