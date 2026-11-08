//! Comment interleaving for both formatter input modes.
//!
//! Parsed source carries comments as a sorted `Vec<Comment>` keyed by byte
//! offset (`attached_to`); a synthetic AST carries `SyntheticComment`s keyed
//! by their anchor node's span start. Both hold owned text, so printing a
//! comment never slices source.
//!
//! The sourced store is a cursor over the sorted array. The synthetic store
//! is a map keyed by anchor: synthesis order needn't match document order,
//! and a synthesizer may comment every statement. It also holds the anchors
//! that want a blank line before them ([`Comments::with_blank_before`]),
//! since there's no source to find blank lines in.

use alloc::collections::BTreeMap;
use core::iter::once;
use core::mem::take;
use core::ops::Range;

use luck_ast::SyntheticComment;
use luck_token::{
    Comment, CommentKind, CommentPosition, CompactString, LuaVersion, Span, index_to_offset,
};
use rustc_hash::FxHashSet;

use crate::ir::{
    Format, FormatElement, Formatter, LineMode, Tag, format_with, hard_line, indent, token,
};

/// A parsed comment, with its text copied out of the source.
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
        /// The dialect `source` was parsed as.
        version: LuaVersion,
        printed_count: usize,
        /// Byte ranges disabled via `-- luck: format off` / `on`.
        disabled_ranges: Vec<Range<u32>>,
    },
    Synthetic {
        /// Keyed by anchor and drained as each anchor is formatted. Whatever
        /// is left flushes at end of file in anchor order.
        comments: BTreeMap<u32, Vec<SyntheticComment>>,
        /// Anchors (statement span starts) that want a blank line before them.
        blank_before_anchors: FxHashSet<u32>,
    },
    Empty,
}

/// Comment state threaded through a format run via `Formatter`.
#[derive(Debug)]
pub struct Comments {
    store: Store,
}

/// Comment progress at a point a speculative region can rewind to.
///
/// Only the parsed path's cursor rewinds. The one speculative region, the
/// verbatim fallback, needs source, and a synthetic comment is taken once.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CommentsCheckpoint {
    printed_count: usize,
}

/// Whether a claimed list writes a separator after its last item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListSeparator {
    /// Table fields: a trailing comma is the expanded-layout convention.
    AfterEachItem,
    /// Call arguments: no dialect's grammar allows a trailing comma.
    BetweenItems,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Directive {
    /// `luck: format off`, which opens a region kept as written.
    FormatOff,
    /// `luck: format on`, which closes it.
    FormatOn,
    /// `luck: ignore` or `luck: format ignore`, which keeps the next
    /// statement as written.
    Ignore,
}

impl Directive {
    fn parse(comment_text: &str) -> Option<Self> {
        let body = comment_text.trim_start_matches('-').trim();
        if body.starts_with("luck: ignore") || body.starts_with("luck: format ignore") {
            Some(Self::Ignore)
        } else if body.starts_with("luck: format off") {
            Some(Self::FormatOff)
        } else if body.starts_with("luck: format on") {
            Some(Self::FormatOn)
        } else {
            None
        }
    }
}

/// The `-- luck: format off` regions, as directive spans.
///
/// Each `off` comment's span is paired with the span of the `on` that closes
/// its region, or `None` when the region runs to end of file. An `off` inside
/// an open region changes nothing.
pub(crate) fn format_off_regions(comments: &[Comment], source: &str) -> Vec<(Span, Option<Span>)> {
    let mut regions = Vec::new();
    let mut unclosed_off: Option<Span> = None;
    for comment in comments {
        match Directive::parse(comment.span.text(source)) {
            Some(Directive::FormatOff) => {
                unclosed_off.get_or_insert(comment.span);
            }
            Some(Directive::FormatOn) => {
                if let Some(off) = unclosed_off.take() {
                    regions.push((off, Some(comment.span)));
                }
            }
            Some(Directive::Ignore) | None => {}
        }
    }
    if let Some(off) = unclosed_off {
        regions.push((off, None));
    }
    regions
}

/// Synthetic text may leave out the `--` marker, so add it when missing.
fn synthetic_comment_text(text: &str) -> CompactString {
    if text.trim_start().starts_with("--") {
        CompactString::from(text)
    } else {
        let mut prefixed = CompactString::from("-- ");
        prefixed.push_str(text);
        prefixed
    }
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
                text: CompactString::from(comment.span.text(source)),
                attached_to: comment.attached_to,
                span: comment.span,
                position: comment.position,
                kind: comment.kind,
            })
            .collect();

        let source_end = index_to_offset(source.len());
        let disabled_ranges = format_off_regions(comments, source)
            .into_iter()
            .map(|(off, on)| off.end..on.map_or(source_end, |close| close.start))
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

    /// Whether the statement anchored at `anchor` asked for a blank line
    /// before it (synthetic path only).
    pub(crate) fn has_synthetic_blank_before(&self, anchor: u32) -> bool {
        match &self.store {
            Store::Synthetic { blank_before_anchors, .. } => blank_before_anchors.contains(&anchor),
            Store::Sourced { .. } | Store::Empty => false,
        }
    }

    /// Original source text, present only on the parsed path.
    pub(crate) fn source_text(&self) -> Option<&str> {
        match &self.store {
            Store::Sourced { source, .. } => Some(source),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    /// The dialect [`Comments::source_text`] was parsed as.
    ///
    /// The verbatim fallback re-lexes its statement with it to tell line
    /// breaks from newlines inside a long string or comment.
    pub(crate) const fn source_version(&self) -> Option<LuaVersion> {
        match &self.store {
            Store::Sourced { version, .. } => Some(*version),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    pub(crate) fn is_format_disabled_at(&self, offset: u32) -> bool {
        match &self.store {
            Store::Sourced { disabled_ranges, .. } => {
                disabled_ranges.iter().any(|range| range.contains(&offset))
            }
            Store::Synthetic { .. } | Store::Empty => false,
        }
    }

    /// Span start of the next unprinted comment (parsed path only), which
    /// blank-line logic uses to bound its gap scans.
    pub(crate) fn peek_next_start(&self) -> Option<u32> {
        match &self.store {
            Store::Sourced { comments, printed_count, .. } => {
                comments.get(*printed_count).map(|comment| comment.span.start)
            }
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    /// Whether an empty region ending at `end` has comments to print.
    ///
    /// On the parsed path those are the unprinted comments before `end` that
    /// can move there; on the synthetic path, any comment anchored at
    /// `anchor` (the block's span start).
    pub(crate) fn has_dangling_comments(&self, anchor: u32, end: u32) -> bool {
        match &self.store {
            Store::Sourced { comments, printed_count, source, .. } => {
                !dangling_run(pending_comments(comments, *printed_count), source, end).is_empty()
            }
            Store::Synthetic { comments, .. } => comments.contains_key(&anchor),
            Store::Empty => false,
        }
    }

    /// Mark the comments in `start..end` printed without emitting them, for a
    /// statement whose verbatim slice already holds them.
    ///
    /// The cursor is sequential, so an unprinted comment before `start` stops
    /// the mark and the ones inside stay pending. They aren't lost:
    /// [`emit_trailing_comments`] won't move a comment from before its
    /// statement, so the enclosing statement finds them still pending and
    /// goes verbatim over its whole span.
    pub(crate) fn mark_printed_range(&mut self, start: u32, end: u32) {
        if let Store::Sourced { comments, printed_count, .. } = &mut self.store {
            take_pending_while(comments, printed_count, |comment| {
                (start..end).contains(&comment.span.start)
            });
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
        self.has_unhandled_in(span.start, span.end)
            && item_spans.all(|item| !self.has_unhandled_in(item.start, item.end))
    }

    /// Whether an unprinted comment starts inside `start..end`.
    pub(crate) fn has_unhandled_in(&self, start: u32, end: u32) -> bool {
        let pending = match &self.store {
            Store::Sourced { comments, printed_count, .. } => {
                pending_comments(comments, *printed_count)
            }
            Store::Synthetic { .. } | Store::Empty => &[],
        };
        pending
            .iter()
            .find(|comment| comment.span.start >= start)
            .is_some_and(|comment| comment.span.start < end)
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
}

/// Emit a shebang line if the document starts with one.
pub(crate) fn emit_shebang(formatter: &mut Formatter) {
    let texts = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, .. } => {
            comment_texts(take_pending_while(comments, printed_count, |comment| {
                comment.kind == CommentKind::Shebang
            }))
        }
        Store::Synthetic { .. } | Store::Empty => Vec::new(),
    };
    push_comment_lines(formatter, texts);
}

/// Emit the leading comments of the statement starting at `anchor`, each on
/// its own line.
///
/// Returns true when one of them is a `luck: ignore` directive.
pub(crate) fn emit_leading_comments(formatter: &mut Formatter, anchor: u32) -> bool {
    let texts = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, .. } => {
            comment_texts(take_pending_while(comments, printed_count, |comment| {
                comment.attached_to == anchor && comment.position == CommentPosition::Leading
            }))
        }
        Store::Synthetic { comments, .. } => {
            take_synthetic(comments, anchor, |comment| comment.is_leading)
        }
        Store::Empty => Vec::new(),
    };
    let has_ignore = texts.iter().any(|text| Directive::parse(text) == Some(Directive::Ignore));
    push_comment_lines(formatter, texts);
    has_ignore
}

/// Emit the comments stalled before the statement starting at `anchor`, each
/// on its own line.
///
/// These are attached to a token no emitter asked for, such as the `then` of
/// an enclosing `if` or a `;` the block loop skips. Left pending, they'd block
/// the cursor for every later comment. The statement's own leading comments
/// end the run, since [`emit_leading_comments`] owns those and reports the
/// `luck: ignore` directive.
///
/// The run moves only if nothing but whitespace and `;` separates its
/// comments from each other and from `anchor`, because crossing a token would
/// relocate a comment. Otherwise it stays pending and the statement holding
/// it falls back to verbatim.
pub(crate) fn emit_stalled_comments(formatter: &mut Formatter, anchor: u32) {
    let texts = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, source, .. } => {
            let pending = pending_comments(comments, *printed_count);
            let candidate_count = pending
                .iter()
                .take_while(|comment| {
                    comment.span.start < anchor
                        && !(comment.attached_to == anchor
                            && comment.position == CommentPosition::Leading)
                })
                .count();
            let (candidates, after_candidates) = pending.split_at(candidate_count);
            // The run has to reach the statement's own leading comment when
            // `take_while` stopped on one, since that prints next, and the
            // anchor otherwise.
            let last_follower =
                after_candidates.first().map_or(anchor, |comment| comment.span.start).min(anchor);
            if is_movable_run(candidates, source, last_follower) {
                *printed_count = printed_count.saturating_add(candidate_count);
                comment_texts(candidates)
            } else {
                Vec::new()
            }
        }
        Store::Synthetic { .. } | Store::Empty => Vec::new(),
    };
    push_comment_lines(formatter, texts);
}

/// Emit the trailing comments of the statement spanning `anchor..stmt_end`,
/// plus any comments inside that span no emitter printed.
///
/// Returns true when a comment went on its own line after the statement. The
/// caller then drops the following blank line: it would sit between that
/// comment and the next statement, and a reparse would read the comment as
/// leading and lose the blank, breaking idempotency.
pub(crate) fn emit_trailing_comments(
    formatter: &mut Formatter,
    anchor: u32,
    stmt_end: u32,
) -> bool {
    enum Placement {
        Suffix(CompactString),
        OwnLine(CompactString),
    }
    let placements: Vec<Placement> = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, source, .. } => {
            // Left pending, comments inside the span would block the cursor
            // for every later comment. One that starts before the statement
            // isn't this statement's to move: it stalled inside an enclosing
            // statement, whose `has_unhandled_in` check sends it verbatim.
            let inner_comments = take_pending_while(comments, printed_count, |comment| {
                (anchor..stmt_end).contains(&comment.span.start)
            });

            let trailing_comments = take_pending_while(comments, printed_count, |comment| {
                if comment.position != CommentPosition::Trailing || stmt_end > comment.span.start {
                    return false;
                }
                // A newline puts the comment on its own line, and a token in
                // between means it trails something else.
                let gap = Span::new(stmt_end, comment.span.start).text(source);
                !gap.contains(['\n', '\r']) && is_code_free(gap)
            });

            // After the inner comments' own lines, a suffix would land on
            // the last one's line, so the trailing comment gets its own too.
            let trailing_placement =
                if inner_comments.is_empty() { Placement::Suffix } else { Placement::OwnLine };
            inner_comments
                .iter()
                .map(|comment| Placement::OwnLine(comment.text.clone()))
                .chain(
                    trailing_comments
                        .iter()
                        .map(|comment| trailing_placement(comment.text.clone())),
                )
                .collect()
        }
        Store::Synthetic { comments, .. } => {
            take_synthetic(comments, anchor, |comment| !comment.is_leading)
                .into_iter()
                .map(Placement::Suffix)
                .collect()
        }
        Store::Empty => Vec::new(),
    };

    let mut has_emitted_own_line = false;
    for placement in placements {
        match placement {
            Placement::Suffix(text) => push_line_suffix(formatter, text),
            Placement::OwnLine(text) => {
                formatter.push(FormatElement::Line(LineMode::Hard));
                formatter.write_text(text);
                has_emitted_own_line = true;
            }
        }
    }
    has_emitted_own_line
}

/// Emit the unprinted comments starting in `start..limit`, each on its own
/// line, for a list that hosts the comments written between its items.
///
/// `start` is the list's own start. A pending comment before it was written
/// outside the list, and taking it would drag it across the opening
/// delimiter, so the cursor stops there and the statement-level fallback
/// takes the whole run.
///
/// Parsed input only: a synthetic comment has no offset to compare, and
/// [`emit_leading_comments`] already places it by anchor.
fn emit_own_line_comments_in(formatter: &mut Formatter, start: u32, limit: u32) {
    let texts = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, .. } => {
            comment_texts(take_pending_while(comments, printed_count, |comment| {
                (start..limit).contains(&comment.span.start)
            }))
        }
        Store::Synthetic { .. } | Store::Empty => Vec::new(),
    };
    for text in texts {
        formatter.push(FormatElement::Line(LineMode::Hard));
        formatter.write_text(text);
    }
}

/// Emit the single-line block comments written right after `offset`,
/// inline and in order, like the `--[[ why ]]` between a condition and its
/// `then`.
///
/// Only whitespace may come before each one, so every comment stays between
/// the same two tokens. A line comment would swallow the rest of the line and
/// a multi-line block comment would break it, so either one ends the run and
/// is left to the statement-level verbatim fallback.
pub(crate) fn emit_inline_comments_after(formatter: &mut Formatter, offset: u32) {
    let texts = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, source, .. } => {
            let mut gap_start = offset;
            comment_texts(take_pending_while(comments, printed_count, |comment| {
                if comment.span.start < gap_start
                    || comment.kind != CommentKind::SingleLineBlock
                    || !is_code_free(Span::new(gap_start, comment.span.start).text(source))
                {
                    return false;
                }
                gap_start = comment.span.end;
                true
            }))
        }
        Store::Synthetic { .. } | Store::Empty => Vec::new(),
    };
    for text in texts {
        formatter.push(FormatElement::Space);
        formatter.write_text(text);
    }
}

/// Emit the comments written on the same line as `offset` as line
/// suffixes, like the `-- why` after a list item's comma.
///
/// The run ends at `limit`, the next item the caller emits. It also ends at a
/// newline, since [`emit_own_line_comments_in`] takes the comments after one,
/// and at a comment pending from before `offset`, which stalled outside
/// this list and isn't the list's to place.
fn emit_same_line_comments_after(formatter: &mut Formatter, offset: u32, limit: u32) {
    let texts = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, source, .. } => {
            let mut gap_start = offset;
            comment_texts(take_pending_while(comments, printed_count, |comment| {
                if comment.span.start < gap_start || comment.span.start >= limit {
                    return false;
                }
                let gap = Span::new(gap_start, comment.span.start).text(source);
                if gap.contains(['\n', '\r']) {
                    return false;
                }
                gap_start = comment.span.end;
                true
            }))
        }
        Store::Synthetic { .. } | Store::Empty => Vec::new(),
    };
    for text in texts {
        push_line_suffix(formatter, text);
    }
}

/// Write a list that holds comments, one item per line, so each comment stays
/// next to the item it was written by.
///
/// A packed or flat layout would push a line-suffix comment past the closing
/// delimiter and out of the list.
///
/// `span` covers the delimiters. [`Comments::has_claimable_comments`] decides
/// whether a list may take this path, and guarantees that every comment here
/// sits inside the delimiters and outside every item.
pub(crate) fn write_commented_list<T>(
    formatter: &mut Formatter,
    delimiters: (&'static str, &'static str),
    span: Span,
    items: &[T],
    span_of: fn(&T) -> Span,
    separator: ListSeparator,
) where
    T: Format,
{
    let (open, close) = delimiters;
    token(open).fmt(formatter);
    indent(format_with(|indented| {
        let mut remaining = items;
        while let [item, rest @ ..] = remaining {
            let item_span = span_of(item);
            emit_own_line_comments_in(indented, span.start, item_span.start);
            hard_line().fmt(indented);
            item.fmt(indented);
            if separator == ListSeparator::AfterEachItem || !rest.is_empty() {
                token(",").fmt(indented);
            }
            let next_start = rest.first().map_or(span.end, |next| span_of(next).start);
            emit_same_line_comments_after(indented, item_span.end, next_start);
            remaining = rest;
        }
        emit_own_line_comments_in(indented, span.start, span.end);
    }))
    .fmt(formatter);
    (hard_line(), token(close)).fmt(formatter);
}

/// Emit the comments in an empty region, such as a function body with no
/// statements, one per line.
///
/// On the parsed path that's the [`dangling_run`] before `end`; on the
/// synthetic path, every comment anchored at `anchor` (the block's span
/// start). The caller writes the line breaks around them.
pub(crate) fn emit_dangling_comments(formatter: &mut Formatter, anchor: u32, end: u32) {
    let texts = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, source, .. } => {
            let run = dangling_run(pending_comments(comments, *printed_count), source, end);
            *printed_count = printed_count.saturating_add(run.len());
            comment_texts(run)
        }
        Store::Synthetic { comments, .. } => take_synthetic(comments, anchor, |_| true),
        Store::Empty => Vec::new(),
    };
    for (index, text) in texts.into_iter().enumerate() {
        if index > 0 {
            formatter.push(FormatElement::Line(LineMode::Hard));
        }
        formatter.write_text(text);
    }
}

/// Flush every comment still unprinted at end of file.
pub(crate) fn emit_remaining_comments(formatter: &mut Formatter, has_stmts_before: bool) {
    let texts = match &mut formatter.comments_mut().store {
        Store::Sourced { comments, printed_count, .. } => {
            let taken = comment_texts(pending_comments(comments, *printed_count));
            *printed_count = comments.len();
            taken
        }
        Store::Synthetic { comments, .. } => take(comments)
            .into_values()
            .flatten()
            .map(|comment| synthetic_comment_text(&comment.text))
            .collect(),
        Store::Empty => Vec::new(),
    };
    for (index, text) in texts.into_iter().enumerate() {
        if index > 0 || has_stmts_before {
            formatter.push(FormatElement::Line(LineMode::Hard));
        }
        formatter.write_text(text);
    }
}

/// Each comment followed by a line break, the shape of comments above a
/// statement.
fn push_comment_lines(formatter: &mut Formatter, texts: Vec<CompactString>) {
    for text in texts {
        formatter.write_text(text);
        formatter.push(FormatElement::Line(LineMode::Hard));
    }
}

/// A comment deferred to the end of the line it trails.
fn push_line_suffix(formatter: &mut Formatter, text: CompactString) {
    formatter.push(FormatElement::Tag(Tag::StartLineSuffix));
    formatter.push(FormatElement::Space);
    formatter.write_text(text);
    formatter.push(FormatElement::Tag(Tag::EndLineSuffix));
}

/// The comments the cursor hasn't reached yet.
fn pending_comments(comments: &[SourcedComment], printed_count: usize) -> &[SourcedComment] {
    comments.get(printed_count..).unwrap_or_default()
}

/// Move the cursor past the leading pending comments matching `should_take`,
/// returning them.
fn take_pending_while<'comments, F>(
    comments: &'comments [SourcedComment],
    printed_count: &mut usize,
    mut should_take: F,
) -> &'comments [SourcedComment]
where
    F: FnMut(&SourcedComment) -> bool,
{
    let pending = pending_comments(comments, *printed_count);
    let count = pending.iter().take_while(|comment| should_take(comment)).count();
    *printed_count = printed_count.saturating_add(count);
    let (taken, _) = pending.split_at(count);
    taken
}

fn comment_texts(comments: &[SourcedComment]) -> Vec<CompactString> {
    comments.iter().map(|comment| comment.text.clone()).collect()
}

/// The comments an empty region ending at `end` may print: all of the
/// pending ones before `end`, or none.
///
/// They print at the region's closing keyword, so the run moves only if
/// nothing but whitespace and `;` separates its comments from each other and
/// from that keyword. An unmovable run, like a comment in an empty function's
/// parameter list that would have to cross the `)`, stays pending, and the
/// statement holding it falls back to verbatim.
fn dangling_run<'comments>(
    pending: &'comments [SourcedComment],
    source: &str,
    end: u32,
) -> &'comments [SourcedComment] {
    let count = pending.iter().take_while(|comment| comment.span.start < end).count();
    let (run, _) = pending.split_at(count);
    if is_movable_run(run, source, end) { run } else { &[] }
}

/// Whether every comment of `run` can reach the next one, and the last can
/// reach `run_end`, crossing only what [`is_code_free`] allows.
fn is_movable_run(run: &[SourcedComment], source: &str, run_end: u32) -> bool {
    let followers = run.iter().skip(1).map(|comment| comment.span.start).chain(once(run_end));
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
fn is_code_free(text: &str) -> bool {
    text.bytes().all(|byte| byte.is_ascii_whitespace() || byte == b';')
}

/// Drain the comments at `anchor` matching `filter`, in insertion order.
fn take_synthetic(
    comments: &mut BTreeMap<u32, Vec<SyntheticComment>>,
    anchor: u32,
    filter: impl Fn(&SyntheticComment) -> bool,
) -> Vec<CompactString> {
    let mut taken = Vec::new();
    if let Some(anchored_comments) = comments.get_mut(&anchor) {
        anchored_comments.retain(|comment| {
            if filter(comment) {
                taken.push(synthetic_comment_text(&comment.text));
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
        Formatter::new(FormatOptions::default(), comments, None)
    }

    #[test]
    fn sourced_leading_taken_once() {
        let source = "-- leading\nlocal x = 1";
        let comments = vec![make_comment(0, 10, 11, CommentKind::Line, CommentPosition::Leading)];
        let mut formatter =
            formatter_over(Comments::from_source(&comments, source, LuaVersion::Lua54));

        assert!(!emit_leading_comments(&mut formatter, 11));
        let first_element_count = formatter.elements().len();
        assert!(first_element_count > 0);
        emit_leading_comments(&mut formatter, 11);
        assert_eq!(formatter.elements().len(), first_element_count);
    }

    #[test]
    fn ignore_directive_detected() {
        let source = "-- luck: ignore\nlocal x = 1";
        let comments = vec![make_comment(0, 15, 16, CommentKind::Line, CommentPosition::Leading)];
        let mut formatter =
            formatter_over(Comments::from_source(&comments, source, LuaVersion::Lua54));
        assert!(emit_leading_comments(&mut formatter, 16));
    }

    #[test]
    fn synthetic_comments_attach_by_anchor() {
        let synthetic = vec![
            SyntheticComment { attached_to: 5, text: "upvalue u0".into(), is_leading: true },
            SyntheticComment {
                attached_to: 5,
                text: "-- explicit marker".into(),
                is_leading: false,
            },
        ];
        let mut formatter = formatter_over(Comments::synthetic(synthetic));

        assert!(!emit_leading_comments(&mut formatter, 5));
        let has_prefixed = formatter
            .elements()
            .iter()
            .any(|element| matches!(element, FormatElement::Text(text) if text == "-- upvalue u0"));
        assert!(has_prefixed, "marker prefix added to bare text");

        emit_trailing_comments(&mut formatter, 5, 0);
        let has_suffix = formatter
            .elements()
            .iter()
            .any(|element| matches!(element, FormatElement::Tag(Tag::StartLineSuffix)));
        assert!(has_suffix);
    }

    #[test]
    fn synthetic_dangling_drained_by_anchor() {
        let synthetic =
            vec![SyntheticComment { attached_to: 9, text: "unreachable".into(), is_leading: true }];
        let comments = Comments::synthetic(synthetic);
        assert!(comments.has_dangling_comments(9, 0));
        assert!(!comments.has_dangling_comments(8, 0));

        let mut formatter = formatter_over(comments);
        emit_dangling_comments(&mut formatter, 9, 0);
        let is_emitted = formatter.elements().iter().any(
            |element| matches!(element, FormatElement::Text(text) if text == "-- unreachable"),
        );
        assert!(is_emitted);
        assert!(!formatter.comments().has_dangling_comments(9, 0));
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
            SyntheticComment { attached_to: 20, text: "second".into(), is_leading: true },
            SyntheticComment { attached_to: 10, text: "first".into(), is_leading: true },
        ];
        let mut formatter = formatter_over(Comments::synthetic(synthetic));
        emit_remaining_comments(&mut formatter, false);
        let texts: Vec<&str> = formatter
            .elements()
            .iter()
            .filter_map(|element| {
                if let FormatElement::Text(text) = element { Some(text.as_str()) } else { None }
            })
            .collect();
        assert_eq!(texts, vec!["-- first", "-- second"]);
    }

    #[test]
    fn format_disabled_ranges() {
        let source = "-- luck: format off\nlocal x=1\n-- luck: format on\nlocal y = 2";
        let source_comments = vec![
            make_comment(0, 19, 20, CommentKind::Line, CommentPosition::Leading),
            make_comment(30, 49, 50, CommentKind::Line, CommentPosition::Leading),
        ];
        let comments = Comments::from_source(&source_comments, source, LuaVersion::Lua54);
        assert!(comments.is_format_disabled_at(25));
        assert!(!comments.is_format_disabled_at(55));
    }
}
