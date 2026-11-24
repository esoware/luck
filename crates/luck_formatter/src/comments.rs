//! A parse carries comments as a sorted `[Comment]` beside its sorted
//! `[Token]`, and a synthetic AST carries `SyntheticComment`s keyed by their
//! anchor node's span start.
//!
//! The parsed store is a cursor over the sorted comments, as `oxc_formatter`'s
//! is. Every pass takes a run from the front of what's left, so comments print
//! in source order and once each, and each lands at the nearest node boundary:
//!
//! - A node's leading comments are all those before it.
//! - Its trailing comments are those right after it on its line, before the
//!   end of the node around it.
//! - A container takes those before its closer, and a block those around its
//!   statements.
//!
//! The parse's tokens say where each token the tree keeps no span for sits,
//! such as a closer, or a `,` a comment trails, so the source is read only for
//! its line breaks and indentation, and for the text that goes out as written.
//!
//! The synthetic store is a map keyed by anchor: synthesis order needn't match
//! document order, and a synthesizer may comment every statement. It also holds
//! the anchors that want a blank line before them
//! ([`Comments::with_blank_before`]), since there's no source to find blank
//! lines in. Only statements and blocks take synthetic comments.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::iter::once;
use std::mem::take;
use std::ops::Range;

use luck_allocator::Allocator;
use luck_ast::{Block, GetSpan as _, Statement};
use luck_parser::ParseResult;
use luck_token::{
    Comment, CommentContent, CommentKind, CommentPosition, LuaVersion, Span, Token, TokenKind,
    has_line_break, index_to_offset, line_breaks,
};
use rustc_hash::{FxBuildHasher, FxHashSet};

use crate::lines::{after_line_break, own_line_start};

/// A comment for a tree built in code, which lives outside the tree, keyed by
/// `attached_to`, the span start of the statement or block it's anchored to.
///
/// The formatter adds `-- ` when `text` doesn't already start with `--`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntheticComment<'text> {
    pub attached_to: u32,
    pub text: &'text str,
    pub position: CommentPosition,
}

impl<'text> SyntheticComment<'text> {
    pub fn leading(stmt: &Statement<'_>, text: &'text str) -> Self {
        Self { attached_to: stmt.span().start, text, position: CommentPosition::Leading }
    }

    pub fn trailing(stmt: &Statement<'_>, text: &'text str) -> Self {
        Self { attached_to: stmt.span().start, text, position: CommentPosition::Trailing }
    }

    /// A comment printed inside `block` when it's empty, such as
    /// `-- unreachable`.
    pub const fn dangling(block: &Block<'_>, text: &'text str) -> Self {
        Self { attached_to: block.span.start, text, position: CommentPosition::Leading }
    }
}

#[derive(Debug)]
struct Parsed<'ast> {
    source: &'ast [u8],
    /// The chunk's dialect, which reads the source's line breaks.
    version: LuaVersion,
    comments: &'ast [Comment],
    tokens: &'ast [Token],
    /// Where the last search of `tokens` stopped, which the next starts from.
    token_cursor: Cell<usize>,
    printed_count: usize,
    /// The spans `-- luck: format off` / `on` regions copy as written,
    /// the directive comments and their own lines included. Sorted, and
    /// empty on almost every file.
    disabled_ranges: Vec<Range<u32>>,
}

#[derive(Debug)]
enum Store<'ast> {
    Parsed(Parsed<'ast>),
    Synthetic {
        /// Keyed by anchor and drained as each anchor is formatted. Whatever
        /// is left flushes at end of file in anchor order.
        comments: BTreeMap<u32, Vec<SyntheticComment<'ast>>>,
        blank_before_anchors: FxHashSet<u32>,
    },
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
    pub const fn none() -> Self {
        Self {
            store: Store::Synthetic {
                comments: BTreeMap::new(),
                blank_before_anchors: FxHashSet::with_hasher(FxBuildHasher),
            },
            next_start: u32::MAX,
        }
    }

    /// `parse` keeps its tokens, as one from `luck_parser::parse_with_tokens`
    /// does.
    pub(crate) fn from_parse(parse: &'ast ParseResult<'ast>) -> Self {
        let (source, comments, tokens) = (parse.source, &*parse.comments, &*parse.tokens);
        debug_assert!(
            !tokens.is_empty(),
            "a parse to format keeps its tokens, as `parse_with_tokens` does"
        );
        let version = parse.chunk.version;
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
        let parsed = Parsed {
            source,
            version,
            comments,
            tokens,
            token_cursor: Cell::new(0),
            printed_count: 0,
            disabled_ranges,
        };
        let next_start = parsed.next_start();
        Self { store: Store::Parsed(parsed), next_start }
    }

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
        if let Store::Synthetic { blank_before_anchors, .. } = &mut self.store {
            blank_before_anchors.extend(anchors);
        }
        self
    }

    /// The parsed path's store, whose source, comments and tokens outlive it.
    const fn parsed(&self) -> Option<&Parsed<'ast>> {
        match &self.store {
            Store::Parsed(parsed) => Some(parsed),
            Store::Synthetic { .. } => None,
        }
    }

    pub(crate) fn has_synthetic_blank_before(&self, anchor: u32) -> bool {
        match &self.store {
            Store::Synthetic { blank_before_anchors, .. } => blank_before_anchors.contains(&anchor),
            Store::Parsed(_) => false,
        }
    }

    pub(crate) const fn source_text(&self) -> Option<&'ast [u8]> {
        match self.parsed() {
            Some(parsed) => Some(parsed.source),
            None => None,
        }
    }

    /// How many tokens and comments the parse holds, which a tree built in
    /// code has none of.
    pub(crate) const fn token_and_comment_count(&self) -> usize {
        match self.parsed() {
            Some(parsed) => parsed.tokens.len() + parsed.comments.len(),
            None => 0,
        }
    }

    /// The first `format off` region not yet behind `position` or the last
    /// printed comment.
    pub(crate) fn next_disabled_region(&self, position: u32) -> Option<Range<u32>> {
        let parsed = self.parsed()?;
        if parsed.disabled_ranges.is_empty() {
            return None;
        }
        let reached = parsed.printed_end().map_or(position, |end| end.max(position));
        region_ending_after(&parsed.disabled_ranges, reached).cloned()
    }

    pub(crate) fn overlaps_disabled_region(&self, span: Span) -> bool {
        self.parsed().is_some_and(|parsed| {
            region_ending_after(&parsed.disabled_ranges, span.start)
                .is_some_and(|region| region.start < span.end)
        })
    }

    /// See [`Parsed::is_code_free`]. Only a parse has tokens to tell.
    pub(crate) fn is_code_free(&self, start: u32, end: u32) -> bool {
        self.parsed().is_some_and(|parsed| parsed.is_code_free(start, end))
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
        self.parsed().map_or(&[], |parsed| parsed.pending_from(start))
    }

    /// Where the first `closer` at or after `from` is, which the tree doesn't
    /// record for every closer. A tree built in code has none to find, so its
    /// closers are all at `from`.
    pub(crate) fn closer_offset(&self, from: u32, closer: TokenKind) -> u32 {
        let Some(parsed) = self.parsed() else {
            return from;
        };
        let tokens = parsed.tokens_from(from);
        tokens.iter().find(|token| token.kind == closer).map_or(from, |token| token.span.start)
    }

    /// The line breaks in `start..end` that part two tokens outside every
    /// comment, where text that goes out as written can take the printer's
    /// indent. One a long string or a long comment holds, or one a `\`
    /// continues a string across, is part of that token, and re-indenting
    /// there would change it.
    pub(crate) fn line_breaks_between_tokens(&self, start: u32, end: u32) -> Vec<Span> {
        let Some(parsed) = self.parsed() else {
            return Vec::new();
        };
        let mut separators = Vec::new();
        let mut add_gap = |gap: Span| {
            let in_gap = line_breaks(gap.text(parsed.source), parsed.version);
            separators.extend(
                in_gap.map(|found| Span::from_indices(found.start, found.end).shifted(gap.start)),
            );
        };
        let all_comments = parsed.comments;
        let first_comment = all_comments.partition_point(|c| c.span.start < start);
        let mut comments = &all_comments[first_comment..];
        let mut gap_start = start;
        let tokens = parsed.tokens_in(start, end).map(|token| token.span);
        for next in tokens.chain(once(Span::new(end, end))) {
            while let Some((comment, rest)) = comments.split_first()
                && comment.span.start < next.start
            {
                add_gap(Span::new(gap_start, comment.span.start));
                gap_start = comment.span.end;
                comments = rest;
            }
            add_gap(Span::new(gap_start, next.start));
            gap_start = next.end;
        }
        separators
    }

    /// Narrow this store to `selection`: comments before it count as printed,
    /// and no `format off` region reaches outside it.
    pub(crate) fn restrict_to(&mut self, selection: Span) {
        if let Store::Parsed(parsed) = &mut self.store {
            let printed_count = parsed.comments.partition_point(|c| c.span.start < selection.start);
            for region in &mut parsed.disabled_ranges {
                region.start = region.start.max(selection.start);
                region.end = region.end.min(selection.end);
            }
            parsed.disabled_ranges.retain(|region| region.start < region.end);
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

    pub(crate) fn last_printed_end(&self) -> Option<u32> {
        self.parsed().and_then(Parsed::printed_end)
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
        self.parsed().map(|parsed| parsed.placeable_before(position)).into_iter().flatten()
    }

    /// Whether a block with no statements, whose span starts at `block_start`,
    /// has comments to print.
    ///
    /// On the parsed path those are the [`Parsed::dangling_run`] there; on the
    /// synthetic path, any comment anchored at `block_start`.
    pub(crate) fn has_dangling_comments(&self, block_start: u32) -> bool {
        match &self.store {
            Store::Parsed(parsed) => !parsed.dangling_run(block_start).is_empty(),
            Store::Synthetic { comments, .. } => comments.contains_key(&block_start),
        }
    }

    /// Mark the comments in `start..end` printed without emitting them, for a
    /// statement whose verbatim slice already holds them.
    pub(crate) fn mark_printed_range(&mut self, start: u32, end: u32) {
        let count = self.parsed().map_or(0, |parsed| {
            parsed.pending().iter().take_while(|c| (start..end).contains(&c.span.start)).count()
        });
        self.take(count);
    }

    /// Takes the comments a pass may place before `position`: those leading
    /// the node there, or those before the closer there.
    pub(crate) fn take_before(&mut self, position: u32) -> &'ast [Comment] {
        let count = self.parsed().map_or(0, |parsed| parsed.placeable_before(position).count());
        self.take(count)
    }

    /// Takes the comments right after a node ending at `end`, on its line and
    /// before `enclosing_end`, where the node around it ends: first those that
    /// trail it in place, then those that go to the end of its line.
    ///
    /// The ones before the first separator trail the node in place. When the
    /// run reaches a comment that ends the line, that one and those between
    /// it and the separators before it go to the line's end. The rest lead
    /// what follows.
    ///
    /// Comments that call arguments or parameters follow lead those instead.
    /// In Lua 5.1 and Luau a line break before a call's `(` makes a new
    /// statement of it, and one moved past a function's `(...)` would lead its
    /// body on the next format.
    pub(crate) fn take_trailing(
        &mut self,
        end: u32,
        enclosing_end: u32,
    ) -> (&'ast [Comment], &'ast [Comment]) {
        let Some((count, in_place_count)) =
            self.parsed().and_then(|parsed| parsed.trailing_run(end, enclosing_end))
        else {
            return (&[], &[]);
        };
        self.take(count).split_at(in_place_count)
    }

    /// Takes the comments after a statement ending at `stmt_end` on its line.
    pub(crate) fn take_stmt_trailing(&mut self, stmt_end: u32) -> &'ast [Comment] {
        let count = self.parsed().map_or(0, |parsed| parsed.stmt_trailing_count(stmt_end));
        self.take(count)
    }

    /// Takes the [`Parsed::dangling_run`] at `start`.
    pub(crate) fn take_dangling(&mut self, start: u32) -> &'ast [Comment] {
        let count = self.parsed().map_or(0, |parsed| parsed.dangling_run(start).len());
        self.take(count)
    }

    pub(crate) fn take_shebang(&mut self) -> &'ast [Comment] {
        let count = self.parsed().map_or(0, |parsed| {
            parsed.pending().iter().take_while(|c| c.kind == CommentKind::Shebang).count()
        });
        self.take(count)
    }

    pub(crate) fn take_remaining(&mut self) -> &'ast [Comment] {
        let count = self.parsed().map_or(0, |parsed| parsed.pending().len());
        self.take(count)
    }

    /// Takes the first `n` unprinted comments.
    fn take(&mut self, n: usize) -> &'ast [Comment] {
        if n == 0 {
            return &[];
        }
        let Store::Parsed(parsed) = &mut self.store else {
            return &[];
        };
        let taken = parsed.take(n);
        self.next_start = parsed.next_start();
        taken
    }

    /// The synthetic comments anchored at `anchor` that `filter` takes, each
    /// line prefixed with `--` where it's missing.
    pub(crate) fn take_synthetic(
        &mut self,
        anchor: u32,
        allocator: &'ast Allocator,
        filter: impl Fn(&SyntheticComment<'_>) -> bool,
    ) -> Vec<&'ast str> {
        let Store::Synthetic { comments, .. } = &mut self.store else {
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

    /// Every synthetic comment not yet taken, in anchor order, as
    /// [`Comments::take_synthetic`] takes them.
    pub(crate) fn take_remaining_synthetic(
        &mut self,
        allocator: &'ast Allocator,
    ) -> Vec<&'ast str> {
        let Store::Synthetic { comments, .. } = &mut self.store else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        for comment in take(comments).into_values().flatten() {
            synthetic_comment_lines(comment.text, allocator, &mut lines);
        }
        lines
    }

    fn set_printed_count(&mut self, printed_count: usize) {
        if let Store::Parsed(parsed) = &mut self.store {
            parsed.printed_count = printed_count;
            self.next_start = parsed.next_start();
        }
    }

    pub(crate) const fn checkpoint(&self) -> CommentsCheckpoint {
        CommentsCheckpoint {
            printed_count: match self.parsed() {
                Some(parsed) => parsed.printed_count,
                None => 0,
            },
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: CommentsCheckpoint) {
        self.set_printed_count(checkpoint.printed_count);
    }
}

impl<'ast> Parsed<'ast> {
    fn pending(&self) -> &'ast [Comment] {
        &self.comments[self.printed_count..]
    }

    fn printed_end(&self) -> Option<u32> {
        self.comments[..self.printed_count].last().map(|c| c.span.end)
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
        &pending[pending.partition_point(|c| c.span.start < start)..]
    }

    /// The tokens from the first at or after `offset` on.
    ///
    /// The formatter asks in source order, nearly always a token or two past
    /// where it last asked, so the search gallops out from there, which takes
    /// a step or two where a search of every token would take a dozen or more.
    fn tokens_from(&self, offset: u32) -> &'ast [Token] {
        let tokens = self.tokens;
        let is_before = |token: &Token| token.span.start < offset;
        let cursor = self.token_cursor.get().min(tokens.len());
        let is_behind = tokens[..cursor].last().is_some_and(|token| !is_before(token));
        let index = if is_behind {
            tokens[..cursor].partition_point(is_before)
        } else {
            // Every token before `low` starts before `offset`, and the first
            // that doesn't is within `step` of it.
            let mut low = cursor;
            let mut step = 1;
            while tokens.get(low + step - 1).is_some_and(is_before) {
                low += step;
                step *= 2;
            }
            low + tokens[low..(low + step).min(tokens.len())].partition_point(is_before)
        };
        self.token_cursor.set(index);
        &tokens[index..]
    }

    /// The tokens that start in `start..end`, read lazily, since a check of a
    /// stretch stops at the first token it can't cross, and the stretch may
    /// run far past it.
    fn tokens_in(&self, start: u32, end: u32) -> impl Iterator<Item = &'ast Token> + use<'ast> {
        self.tokens_from(start).iter().take_while(move |token| token.span.start < end)
    }

    /// Whether no token but a `;` starts in `start..end`, so a comment can
    /// cross the stretch and still sit between the same two tokens, since the
    /// formatter writes statement separators as it sees fit.
    fn is_code_free(&self, start: u32, end: u32) -> bool {
        self.tokens_in(start, end).all(|token| token.kind == TokenKind::Semicolon)
    }

    /// The comments a block may print with no statement left to attach them
    /// to.
    ///
    /// That's those pending before `start`, where the block's remaining text
    /// begins, or none if any can't reach it, then the run after it up to the
    /// next token. `start` is the closer of an empty block or the end of its
    /// last statement.
    ///
    /// No token but a `;` may part the run's comments from each other or from
    /// `start`, and the run stops at the next `format off` region.
    fn dangling_run(&self, start: u32) -> &'ast [Comment] {
        let pending = self.pending();
        let before_count = pending.iter().take_while(|c| c.span.start < start).count();
        let (before, after) = pending.split_at(before_count);
        let followers = before.iter().skip(1).map(|c| c.span.start).chain(once(start));
        let can_move = before.iter().zip(followers).all(|(comment, next_start)| {
            next_start >= comment.span.end && self.is_code_free(comment.span.end, next_start)
        });
        if !can_move {
            return &[];
        }
        let limit = self.disabled_region_limit();
        let mut gap_start = start;
        let after_count = after
            .iter()
            .take_while(|comment| {
                let is_reachable =
                    comment.span.start < limit && self.is_code_free(gap_start, comment.span.start);
                gap_start = comment.span.end;
                is_reachable
            })
            .count();
        &pending[..before_count + after_count]
    }

    /// How many of the comments after a node ending at `end` and before
    /// `enclosing_end` [`Comments::take_trailing`] takes, and how many of
    /// those trail the node in place, or `None` for none.
    fn trailing_run(&self, end: u32, enclosing_end: u32) -> Option<(usize, usize)> {
        let mut gap_start = end;
        let mut in_place_count = 0;
        let mut run_length = 0;
        let mut in_place_end = end;
        let mut has_separator = false;
        let mut is_line_ending = false;
        // The separators the formatter writes itself are all a comment may
        // cross after its node.
        let is_separator =
            |token: &Token| matches!(token.kind, TokenKind::Comma | TokenKind::Semicolon);
        for comment in self.placeable_before(enclosing_end) {
            if comment.span.start < gap_start {
                return None;
            }
            let mut crossed = self.tokens_in(gap_start, comment.span.start).peekable();
            let does_cross = crossed.peek().is_some();
            let gap = Span::new(gap_start, comment.span.start).text(self.source);
            if !crossed.all(is_separator) || has_line_break(gap, self.version) {
                break;
            }
            has_separator |= does_cross;
            run_length += 1;
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
        if count == 0 {
            return None;
        }
        let next_token = self.tokens_from(last_end).first();
        if !has_separator && next_token.is_some_and(|token| starts_call_arguments(token.kind)) {
            return None;
        }
        Some((count, in_place_count))
    }

    /// How many comments follow a statement ending at `stmt_end` on its line.
    fn stmt_trailing_count(&self, stmt_end: u32) -> usize {
        let mut gap_start = stmt_end;
        // Luau ends a line comment at a lone CR but reads no line break
        // there, so a comment after one is on the same line, where it can't
        // print without joining it.
        let mut is_after_line_comment = false;
        self.placeable_before(u32::MAX)
            .take_while(|comment| {
                if is_after_line_comment || gap_start > comment.span.start {
                    return false;
                }
                let gap = Span::new(gap_start, comment.span.start).text(self.source);
                let is_on_the_line = self.is_code_free(gap_start, comment.span.start)
                    && !has_line_break(gap, self.version);
                gap_start = comment.span.end;
                is_after_line_comment = comment.kind == CommentKind::Line;
                is_on_the_line
            })
            .count()
    }

    /// Takes the first `n` pending comments, which only
    /// [`Comments::take`] calls, since it keeps `next_start` up to date.
    fn take(&mut self, n: usize) -> &'ast [Comment] {
        let pending = self.pending();
        self.printed_count += n;
        &pending[..n]
    }
}

/// Whether `comment` ends its line, so nothing may follow it there.
pub(crate) fn ends_line(comment: &Comment) -> bool {
    comment.kind == CommentKind::Line || comment.is_followed_by_newline
}

/// Whether a token of `kind` starts call arguments or parameters: a `(`, a
/// table, or a string.
const fn starts_call_arguments(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::LeftParen | TokenKind::LeftBrace | TokenKind::StringLiteral)
}

/// The first of `regions` that ends past `position`. `format off` regions are
/// sorted and apart, so their ends are in order too.
fn region_ending_after(regions: &[Range<u32>], position: u32) -> Option<&Range<u32>> {
    regions.get(regions.partition_point(|region| region.end <= position))
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

pub(crate) fn is_long_comment(text: &str) -> bool {
    text.trim_start()
        .strip_prefix("--[")
        .is_some_and(|rest| rest.trim_start_matches('=').starts_with('['))
}

#[cfg(test)]
mod tests {
    use std::iter;

    use super::*;
    use crate::fixtures::parse;

    #[test]
    fn call_arguments_start_at_a_paren_a_table_or_a_string() {
        let allocator = Allocator::new();
        for (source, expected) in [
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
            let first = parse(&allocator, source).tokens.first().map(|token| token.kind);
            assert_eq!(first.is_some_and(starts_call_arguments), expected, "{source:?}");
        }
    }

    #[test]
    fn tokens_are_found_from_any_cursor() {
        let allocator = Allocator::new();
        let source = "local a = f(b, {[c] = 1}) -- d\nreturn a";
        let parsed = parse(&allocator, source);
        let Comments { store: Store::Parsed(store), .. } = Comments::from_parse(&parsed) else {
            panic!("a parsed store");
        };
        let end = index_to_offset(source.len());
        for start_cursor in 0..=parsed.tokens.len() + 2 {
            store.token_cursor.set(start_cursor);
            for offset in (0..=end).chain((0..=end).rev()) {
                let first = parsed.tokens.partition_point(|token| token.span.start < offset);
                let expected = &parsed.tokens[first..];
                assert_eq!(store.tokens_from(offset), expected, "{offset} from {start_cursor}");
            }
        }
    }

    #[test]
    fn a_closer_is_found_past_the_comments_that_hold_one() {
        let allocator = Allocator::new();
        let parsed = parse(&allocator, "a --[[)]] -- )\n)");
        let store = Comments::from_parse(&parsed);
        assert_eq!(store.closer_offset(1, TokenKind::RightParen), 15);
        assert_eq!(Comments::none().closer_offset(1, TokenKind::RightParen), 1);
    }

    #[test]
    fn line_breaks_inside_a_token_or_a_comment_do_not_part_tokens() {
        let allocator = Allocator::new();
        for (source, expected) in [
            ("a\nb\nc", vec![1, 3]),
            // Long string, long comment, and level-2 bracket.
            ("a\n[[b\nc]]\nd", vec![1, 9]),
            ("a\n--[[b\nc]]\nd", vec![1, 11]),
            ("a\n[==[b\nc]==]\nd", vec![1, 13]),
            // A level mismatch doesn't close the bracket.
            ("[==[a\nb]=]\nc", vec![]),
            // `]]` without an opener is just two tokens.
            ("a]]\nb", vec![3]),
            // A short string continued across the break, and `\z`.
            ("\"a\\\nb\"\nc", vec![6]),
            ("\"a\\z\n  b\"\nc", vec![9]),
            // A trailing `\` in a comment escapes nothing, and `[[` inside a
            // short string opens nothing.
            ("a -- see C:\\\nb", vec![12]),
            ("\"[[\"\nb", vec![4]),
            ("a\r\n\r\nb", vec![1, 3]),
        ] {
            let parsed = parse(&allocator, source);
            let end = index_to_offset(source.len());
            let found = Comments::from_parse(&parsed)
                .line_breaks_between_tokens(0, end)
                .iter()
                .map(|line_break| line_break.start)
                .collect::<Vec<_>>();
            assert_eq!(found, expected, "{source:?}");
        }
    }

    #[test]
    fn a_line_ending_comment_outside_every_item_is_found() {
        let allocator = Allocator::new();
        let parsed = parse(&allocator, "(a --[[x]]\n, b --[[z]]) --[[y]]");
        let store = Comments::from_parse(&parsed);
        let call = Span::new(0, 23);
        let has_outside =
            |items: &[Span]| store.has_line_ending_comment_outside(call, items.iter().copied());
        assert!(has_outside(&[Span::new(1, 2), Span::new(13, 14)]));
        assert!(!has_outside(&[Span::new(1, 10)]), "inside an item");
        assert!(!store.has_line_ending_comment_outside(Span::new(11, 23), iter::empty()));
    }

    #[test]
    fn synthetic_blank_before_recorded() {
        let comments = Comments::synthetic([]).with_blank_before([3, 7]);
        assert!(comments.has_synthetic_blank_before(3));
        assert!(comments.has_synthetic_blank_before(7));
        assert!(!comments.has_synthetic_blank_before(5));

        let from_none = Comments::none().with_blank_before([2]);
        assert!(from_none.has_synthetic_blank_before(2));
    }

    #[test]
    fn a_format_off_region_ends_at_format_on() {
        let allocator = Allocator::new();
        let source = "\
-- luck: format off
local x=1
-- luck: format on
local y = 2";
        let parsed = parse(&allocator, source);
        let comments = Comments::from_parse(&parsed);
        assert!(comments.overlaps_disabled_region(Span::new(25, 26)));
        assert!(!comments.overlaps_disabled_region(Span::new(55, 56)));
    }
}
