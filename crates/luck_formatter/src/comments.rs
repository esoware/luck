//! Comment interleaving for both formatter input modes.
//!
//! Parsed source carries comments as a sorted `Vec<Comment>` keyed by byte
//! offsets (`attached_to`); a synthetic AST carries
//! `SyntheticComment`s keyed by the anchor node's span start. Both resolve
//! to owned text at construction, so emission never slices source.
//!
//! The sourced store is a cursor over the sorted array (same model the old
//! formatter used). The synthetic store is a map keyed by anchor, because
//! synthesis order does not guarantee document order and a synthesizer may
//! attach comments to every statement - per-anchor lookup has to stay cheap
//! at that density. The synthetic store additionally carries the set of
//! anchors that want a blank line before them ([`Comments::with_blank_before`]),
//! the source-less replacement for blank-line preservation.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};

use compact_str::CompactString;
use luck_ast::synth::SyntheticComment;
use luck_token::comment::{Comment, CommentKind, CommentPosition};
use luck_token::{LuaVersion, Span};

use crate::ir::{
    Format, FormatElement, Formatter, LineMode, Tag, format_with, hard_line, indent, token,
};

/// A comment with resolved text, uniform across both input modes.
#[derive(Debug, Clone)]
struct SourcedEntry {
    text: CompactString,
    attached_to: u32,
    span_start: u32,
    span_end: u32,
    position: CommentPosition,
    kind: CommentKind,
}

#[derive(Debug)]
enum Store {
    Sourced {
        entries: Vec<SourcedEntry>,
        /// Kept for newline-gap checks and verbatim statement emission.
        source: String,
        /// The dialect `source` was parsed as, so a verbatim region can
        /// re-lex it with the same rules the spans came from.
        version: LuaVersion,
        printed: usize,
        /// Byte ranges disabled via `-- luck: format off` / `on`.
        disabled_ranges: Vec<(u32, u32)>,
    },
    Synthetic {
        /// Anchor-keyed; entries drain as their anchors are formatted, and
        /// whatever remains flushes at end of file in anchor order.
        entries: BTreeMap<u32, Vec<SyntheticComment>>,
        /// Total entries still unprinted, for checkpoint/restore.
        remaining: usize,
        /// Anchors (statement span starts) that want a blank line before them.
        blank_before: HashSet<u32>,
    },
    Empty,
}

/// Comment state threaded through a format run via `Formatter`.
#[derive(Debug)]
pub struct Comments {
    store: Store,
}

/// Everything a speculative region may consume, rewound as one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommentsCheckpoint {
    printed: usize,
}

/// Whether a claimed list writes a separator after its last item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListSeparator {
    /// Table fields: a trailing comma is the expanded-layout convention.
    AfterEachItem,
    /// Call arguments: no dialect's grammar allows a trailing comma.
    BetweenItems,
}

/// Whether the comment text carries a `luck: ignore` /
/// `luck: format ignore` directive for the following statement.
fn is_ignore_directive(text: &str) -> bool {
    let trimmed = text.trim_start_matches('-').trim();
    trimmed.starts_with("luck: ignore") || trimmed.starts_with("luck: format ignore")
}

/// Synthetic comment text may or may not include the comment marker;
/// normalize so emitters always print valid Lua.
fn synthetic_comment_text(text: &str) -> CompactString {
    if text.trim_start().starts_with("--") {
        CompactString::from(text)
    } else {
        let mut owned = CompactString::from("-- ");
        owned.push_str(text);
        owned
    }
}

impl Comments {
    pub fn none() -> Self {
        Self {
            store: Store::Empty,
        }
    }

    pub fn from_source(comments: &[Comment], source: &str, version: LuaVersion) -> Self {
        let entries = comments
            .iter()
            .map(|comment| SourcedEntry {
                text: CompactString::from(
                    &source[comment.span.start as usize..comment.span.end as usize],
                ),
                attached_to: comment.attached_to,
                span_start: comment.span.start,
                span_end: comment.span.end,
                position: comment.position,
                kind: comment.kind,
            })
            .collect::<Vec<_>>();

        let mut disabled_ranges = Vec::new();
        let mut off_start: Option<u32> = None;
        for entry in &entries {
            let trimmed = entry.text.trim_start_matches('-').trim();
            if trimmed.starts_with("luck: format off") {
                if off_start.is_none() {
                    off_start = Some(entry.span_end);
                }
            } else if trimmed.starts_with("luck: format on")
                && let Some(start) = off_start.take()
            {
                disabled_ranges.push((start, entry.span_start));
            }
        }
        if let Some(start) = off_start {
            disabled_ranges.push((start, source.len() as u32));
        }

        Self {
            store: Store::Sourced {
                entries,
                source: source.to_string(),
                version,
                printed: 0,
                disabled_ranges,
            },
        }
    }

    pub fn synthetic(comments: Vec<SyntheticComment>) -> Self {
        let remaining = comments.len();
        let mut entries: BTreeMap<u32, Vec<SyntheticComment>> = BTreeMap::new();
        for comment in comments {
            entries
                .entry(comment.attached_to)
                .or_default()
                .push(comment);
        }
        Self {
            store: Store::Synthetic {
                entries,
                remaining,
                blank_before: HashSet::new(),
            },
        }
    }

    /// Request a blank line before each statement whose span start is in
    /// `anchors` - the source-less way to separate logical regions.
    ///  Only meaningful for synthetic input; on
    /// the parsed path blank lines come from the source itself.
    #[must_use]
    pub fn with_blank_before(mut self, anchors: impl IntoIterator<Item = u32>) -> Self {
        if matches!(self.store, Store::Empty) {
            self.store = Store::Synthetic {
                entries: BTreeMap::new(),
                remaining: 0,
                blank_before: HashSet::new(),
            };
        }
        if let Store::Synthetic { blank_before, .. } = &mut self.store {
            blank_before.extend(anchors);
        }
        self
    }

    /// Whether the statement anchored at `anchor` asked for a blank line
    /// before it (synthetic path only).
    pub(crate) fn has_synthetic_blank_before(&self, anchor: u32) -> bool {
        match &self.store {
            Store::Synthetic { blank_before, .. } => blank_before.contains(&anchor),
            Store::Sourced { .. } | Store::Empty => false,
        }
    }

    /// Original source text, present only for the parsed path. Verbatim
    /// regions (`format off`, `luck: ignore`) require it.
    pub fn source_text(&self) -> Option<&str> {
        match &self.store {
            Store::Sourced { source, .. } => Some(source),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    /// The dialect [`Comments::source_text`] was parsed as. Verbatim regions
    /// re-lex their slice to tell the newlines that separate lines from the
    /// ones a long string or comment carries as content.
    pub(crate) fn source_version(&self) -> Option<LuaVersion> {
        match &self.store {
            Store::Sourced { version, .. } => Some(*version),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    pub fn is_format_disabled_at(&self, pos: u32) -> bool {
        match &self.store {
            Store::Sourced {
                disabled_ranges, ..
            } => disabled_ranges
                .iter()
                .any(|&(start, end)| pos >= start && pos < end),
            Store::Synthetic { .. } | Store::Empty => false,
        }
    }

    /// Span start of the next unprinted comment (sourced path only);
    /// blank-line logic uses it to bound gap scans.
    pub fn peek_next_start(&self) -> Option<u32> {
        match &self.store {
            Store::Sourced {
                entries, printed, ..
            } => entries.get(*printed).map(|entry| entry.span_start),
            Store::Synthetic { .. } | Store::Empty => None,
        }
    }

    /// Whether an empty region ending at `end` holds comments: on the parsed
    /// path the unprinted comments before `end` that may move there, on the
    /// synthetic path any comment anchored to the region itself at `anchor`
    /// (a block span start).
    pub fn has_dangling_comments(&self, anchor: u32, end: u32) -> bool {
        match &self.store {
            Store::Sourced {
                entries,
                printed,
                source,
                ..
            } => dangling_run(&entries[*printed..], source, end) > 0,
            Store::Synthetic { entries, .. } => entries.contains_key(&anchor),
            Store::Empty => false,
        }
    }

    /// Mark comments in `start..end` as printed without emitting them - used
    /// when a statement is emitted verbatim from source, where the slice
    /// already contains its inner comments. The cursor is sequential, so a
    /// comment left unprinted before `start` stops the mark short and the
    /// interior ones stay pending; they are not lost, because
    /// [`Formatter::emit_trailing_comments`] refuses to relocate anything
    /// before its statement and the enclosing statement then goes verbatim
    /// over the whole range.
    pub(crate) fn mark_printed_range(&mut self, start: u32, end: u32) {
        if let Store::Sourced {
            entries, printed, ..
        } = &mut self.store
        {
            while let Some(entry) = entries.get(*printed) {
                if start <= entry.span_start && entry.span_start < end {
                    *printed += 1;
                } else {
                    break;
                }
            }
        }
    }

    /// Start offsets of the unprinted comments inside `start..end`, in source
    /// order. Emitters that claim their own interior comments use it to decide
    /// up front whether every one of them lands somewhere they can print it.
    pub(crate) fn pending_starts_in(&self, start: u32, end: u32) -> impl Iterator<Item = u32> + '_ {
        let entries: &[SourcedEntry] = match &self.store {
            Store::Sourced {
                entries, printed, ..
            } => &entries[*printed..],
            Store::Synthetic { .. } | Store::Empty => &[],
        };
        entries
            .iter()
            .map(|entry| entry.span_start)
            .skip_while(move |offset| *offset < start)
            .take_while(move |offset| *offset < end)
    }

    /// Whether a list-shaped construct covering `span` should claim its own
    /// comments: there is at least one inside, and every one of them sits in
    /// a gap between `item_spans`, where a one-item-per-line layout can host
    /// it. A comment *within* an item belongs to that item's emitter, or, if
    /// none claims it, to the statement-level verbatim fallback.
    pub(crate) fn has_claimable_comments(
        &self,
        span: Span,
        item_spans: impl Iterator<Item = Span> + Clone,
    ) -> bool {
        // A comment still pending from before the construct stalls the
        // sequential cursor, so the ones inside cannot be printed either.
        if self
            .peek_next_start()
            .is_none_or(|start| start < span.start)
        {
            return false;
        }
        let mut offsets = self.pending_starts_in(span.start, span.end).peekable();
        offsets.peek().is_some()
            && offsets.all(|offset| {
                !item_spans
                    .clone()
                    .any(|item| item.start <= offset && offset < item.end)
            })
    }

    pub(crate) fn has_unhandled_in(&self, start: u32, end: u32) -> bool {
        self.pending_starts_in(start, end).next().is_some()
    }

    pub(crate) fn checkpoint(&self) -> CommentsCheckpoint {
        CommentsCheckpoint {
            printed: match &self.store {
                Store::Sourced { printed, .. } => *printed,
                Store::Synthetic { remaining, .. } => *remaining,
                Store::Empty => 0,
            },
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: CommentsCheckpoint) {
        match &mut self.store {
            Store::Sourced { printed, .. } => *printed = checkpoint.printed,
            Store::Synthetic { remaining, .. } => {
                debug_assert!(
                    *remaining == checkpoint.printed,
                    "synthetic comments were taken inside a speculative region"
                );
            }
            Store::Empty => {}
        }
    }
}

impl Formatter {
    /// Emit a shebang line if the document starts with one.
    pub fn emit_shebang(&mut self) {
        let element = match &mut self.comments.store {
            Store::Sourced {
                entries, printed, ..
            } => match entries.get(*printed) {
                Some(entry) if entry.kind == CommentKind::Shebang => {
                    let text = entry.text.clone();
                    *printed += 1;
                    Some(text)
                }
                _ => None,
            },
            Store::Synthetic { .. } | Store::Empty => None,
        };
        if let Some(text) = element {
            self.push(FormatElement::Text(text));
            self.push(FormatElement::Line(LineMode::Hard));
        }
    }

    /// Emit comments leading the token/statement starting at `anchor`.
    /// Returns true when one of them is a `luck: ignore` directive for the
    /// following statement.
    pub fn emit_leading_comments(&mut self, anchor: u32) -> bool {
        let texts: Vec<CompactString> = match &mut self.comments.store {
            Store::Sourced {
                entries, printed, ..
            } => {
                let mut taken = Vec::new();
                while let Some(entry) = entries.get(*printed) {
                    if entry.attached_to == anchor && entry.position == CommentPosition::Leading {
                        taken.push(entry.text.clone());
                        *printed += 1;
                    } else {
                        break;
                    }
                }
                taken
            }
            Store::Synthetic {
                entries, remaining, ..
            } => take_synthetic(entries, remaining, anchor, |entry| entry.is_leading),
            Store::Empty => Vec::new(),
        };

        let mut has_ignore = false;
        for text in texts {
            has_ignore |= is_ignore_directive(&text);
            self.push(FormatElement::Text(text));
            self.push(FormatElement::Line(LineMode::Hard));
        }
        has_ignore
    }

    /// Emit comments that stalled before the statement starting at `anchor` -
    /// ones attached to a token no emitter asked for, such as a comment on
    /// the `then` of an enclosing `if` or on a `;` the block loop skips.
    /// Each lands on its own line, keeping source order. The statement's own
    /// leading comments stop the drain: [`Formatter::emit_leading_comments`]
    /// owns those, and only it reports the `luck: ignore` directive.
    ///
    /// Left unprinted they would stall the sequential cursor and every later
    /// comment would drift with them - but only a run separated from `anchor`
    /// by nothing but whitespace and `;` may move, since crossing a token
    /// would relocate the comment. An unmovable run stays put, and the
    /// statement holding it falls back to verbatim.
    pub fn emit_stalled_comments(&mut self, anchor: u32) {
        let texts: Vec<CompactString> = match &mut self.comments.store {
            Store::Sourced {
                entries,
                printed,
                source,
                ..
            } => {
                let candidates: Vec<&SourcedEntry> = entries[*printed..]
                    .iter()
                    .take_while(|entry| {
                        entry.span_start < anchor
                            && !(entry.attached_to == anchor
                                && entry.position == CommentPosition::Leading)
                    })
                    .collect();
                // Each comment is followed by the next one, and the last by
                // whatever comes after the run - the statement's own leading
                // comment when `take_while` stopped on one, since
                // `emit_leading_comments` prints it next, otherwise the
                // anchor. Every one of those gaps has to be crossable for the
                // run to move as a unit.
                let last_follower = entries
                    .get(*printed + candidates.len())
                    .map_or(anchor, |entry| entry.span_start)
                    .min(anchor);
                let followers = candidates
                    .iter()
                    .skip(1)
                    .map(|entry| entry.span_start)
                    .chain(std::iter::once(last_follower));
                let is_movable = candidates.iter().zip(followers).all(|(entry, next_start)| {
                    next_start >= entry.span_end
                        && is_code_free(&source[entry.span_end as usize..next_start as usize])
                });
                if is_movable {
                    let taken = candidates.iter().map(|entry| entry.text.clone()).collect();
                    *printed += candidates.len();
                    taken
                } else {
                    Vec::new()
                }
            }
            Store::Synthetic { .. } | Store::Empty => Vec::new(),
        };
        for text in texts {
            self.push(FormatElement::Text(text));
            self.push(FormatElement::Line(LineMode::Hard));
        }
    }

    /// Emit trailing comments for the statement spanning `anchor..stmt_end`,
    /// plus any comments that lived inside that span which no emitter visited.
    ///
    /// Returns true when a comment was relocated onto its own line after the
    /// statement: a following blank line would then sit between that comment
    /// and the next statement, where reparse reads it as a leading comment and
    /// drops the blank - so the caller suppresses it to stay idempotent.
    pub fn emit_trailing_comments(&mut self, anchor: u32, stmt_end: u32) -> bool {
        enum Placement {
            Suffix(CompactString),
            OwnLine(CompactString),
        }
        let placements: Vec<Placement> = match &mut self.comments.store {
            Store::Sourced {
                entries,
                printed,
                source,
                ..
            } => {
                let mut placements = Vec::new();

                // Drain comments inside the statement's span first (tables,
                // call args, chains). Left unprinted they would stall the
                // cursor and relocate every later comment to EOF. A comment
                // that starts before the statement is not this statement's to
                // move: it stalled inside an enclosing construct's header, and
                // that statement's `has_unhandled_in` check turns it verbatim.
                let mut has_inner = false;
                while let Some(entry) = entries.get(*printed) {
                    if entry.span_start < anchor || entry.span_start >= stmt_end {
                        break;
                    }
                    placements.push(Placement::OwnLine(entry.text.clone()));
                    has_inner = true;
                    *printed += 1;
                }

                while let Some(entry) = entries.get(*printed) {
                    if entry.position != CommentPosition::Trailing || stmt_end > entry.span_start {
                        break;
                    }
                    // A newline puts the comment on its own line, and any
                    // token between the two means it trails something else.
                    let gap = &source[stmt_end as usize..entry.span_start as usize];
                    if gap.contains(['\n', '\r']) || !is_code_free(gap) {
                        break;
                    }
                    if has_inner {
                        // Inner comments already forced their own lines; a
                        // suffix would print after them, so the trailing
                        // comment joins the standalone run.
                        placements.push(Placement::OwnLine(entry.text.clone()));
                    } else {
                        placements.push(Placement::Suffix(entry.text.clone()));
                    }
                    *printed += 1;
                }
                placements
            }
            Store::Synthetic {
                entries, remaining, ..
            } => take_synthetic(entries, remaining, anchor, |entry| !entry.is_leading)
                .into_iter()
                .map(Placement::Suffix)
                .collect(),
            Store::Empty => Vec::new(),
        };

        let mut emitted_own_line = false;
        for placement in placements {
            match placement {
                Placement::Suffix(text) => {
                    self.push(FormatElement::Tag(Tag::StartLineSuffix));
                    self.push(FormatElement::Space);
                    self.push(FormatElement::Text(text));
                    self.push(FormatElement::Tag(Tag::EndLineSuffix));
                }
                Placement::OwnLine(text) => {
                    self.push(FormatElement::Line(LineMode::Hard));
                    self.push(FormatElement::Text(text));
                    emitted_own_line = true;
                }
            }
        }
        emitted_own_line
    }

    /// Emit every unprinted comment starting in `start..limit`, each preceded
    /// by a hard line break so it lands on its own line. For emitters that
    /// claim the comments inside their own span - a list whose items break one
    /// per line can host a comment written in the gap between two of them.
    ///
    /// `start` is the claiming construct's own start. A comment before it was
    /// written outside the construct, so taking it would carry it across the
    /// opening delimiter; the sequential cursor stops there and leaves the
    /// whole run to the statement-level fallback.
    ///
    /// Sourced input only: a synthetic comment has no offset to compare, and
    /// [`Formatter::emit_leading_comments`] already places it by anchor.
    pub fn emit_own_line_comments_in(&mut self, start: u32, limit: u32) {
        let texts: Vec<CompactString> = match &mut self.comments.store {
            Store::Sourced {
                entries, printed, ..
            } => {
                let mut taken = Vec::new();
                while let Some(entry) = entries.get(*printed) {
                    if entry.span_start < start || entry.span_start >= limit {
                        break;
                    }
                    taken.push(entry.text.clone());
                    *printed += 1;
                }
                taken
            }
            Store::Synthetic { .. } | Store::Empty => Vec::new(),
        };
        for text in texts {
            self.push(FormatElement::Line(LineMode::Hard));
            self.push(FormatElement::Text(text));
        }
    }

    /// Emit the single-line block comments written immediately after
    /// `position`, inline, in the tokens' own order - the `--[[ why ]]`
    /// between a condition and its `then`. Only whitespace may separate the
    /// run from `position`, so each comment prints between the same two
    /// tokens it was written between.
    ///
    /// A line comment would swallow the rest of the line and a multi-line
    /// block comment would break it, so both stop the run and fall through to
    /// the statement-level verbatim fallback.
    pub fn emit_inline_comments_after(&mut self, position: u32) {
        let texts: Vec<CompactString> = match &mut self.comments.store {
            Store::Sourced {
                entries,
                printed,
                source,
                ..
            } => {
                let mut taken = Vec::new();
                let mut gap_start = position;
                while let Some(entry) = entries.get(*printed) {
                    if entry.span_start < gap_start
                        || entry.kind != CommentKind::SingleLineBlock
                        || !is_code_free(&source[gap_start as usize..entry.span_start as usize])
                    {
                        break;
                    }
                    taken.push(entry.text.clone());
                    gap_start = entry.span_end;
                    *printed += 1;
                }
                taken
            }
            Store::Synthetic { .. } | Store::Empty => Vec::new(),
        };
        for text in texts {
            self.push(FormatElement::Space);
            self.push(FormatElement::Text(text));
        }
    }

    /// Emit the comments written on the same source line as `position`, as
    /// line suffixes - the `-- why` after a list item's comma. A newline in
    /// the gap ends the run: those belong to the following line, and
    /// [`Formatter::emit_own_line_comments_in`] takes them instead.
    /// `limit` bounds the run at the next item the caller will emit.
    ///
    /// A comment still pending from before `position` stalled outside this
    /// list and is not the list's to place, so it ends the run too.
    pub fn emit_same_line_comments_after(&mut self, position: u32, limit: u32) {
        let texts: Vec<CompactString> = match &mut self.comments.store {
            Store::Sourced {
                entries,
                printed,
                source,
                ..
            } => {
                let mut taken = Vec::new();
                let mut gap_start = position;
                while let Some(entry) = entries.get(*printed) {
                    if entry.span_start < gap_start || entry.span_start >= limit {
                        break;
                    }
                    let gap = &source[gap_start as usize..entry.span_start as usize];
                    if gap.contains(['\n', '\r']) {
                        break;
                    }
                    taken.push(entry.text.clone());
                    gap_start = entry.span_end;
                    *printed += 1;
                }
                taken
            }
            Store::Synthetic { .. } | Store::Empty => Vec::new(),
        };
        for text in texts {
            self.push(FormatElement::Tag(Tag::StartLineSuffix));
            self.push(FormatElement::Space);
            self.push(FormatElement::Text(text));
            self.push(FormatElement::Tag(Tag::EndLineSuffix));
        }
    }

    /// Layout for a list carrying comments: always expanded, one item per
    /// line, so a comment keeps the line it was written on. Packing or
    /// flattening would defer a line-suffix comment past the closing
    /// delimiter, moving it out of the list entirely.
    ///
    /// `span` covers the delimiters; every comment placed here is one written
    /// inside them, in a gap between two items -
    /// [`Comments::has_claimable_comments`] is what decides a list may take
    /// this path.
    pub(crate) fn write_commented_list<T: Format>(
        &mut self,
        delimiters: (&'static str, &'static str),
        span: Span,
        items: &[T],
        span_of: fn(&T) -> Span,
        separator: ListSeparator,
    ) {
        let (open, close) = delimiters;
        token(open).fmt(self);
        indent(format_with(|f| {
            for (index, item) in items.iter().enumerate() {
                let item_span = span_of(item);
                f.emit_own_line_comments_in(span.start, item_span.start);
                hard_line().fmt(f);
                item.fmt(f);
                if separator == ListSeparator::AfterEachItem || index + 1 < items.len() {
                    token(",").fmt(f);
                }
                let next_start = items
                    .get(index + 1)
                    .map_or(span.end, |next| span_of(next).start);
                f.emit_same_line_comments_after(item_span.end, next_start);
            }
            f.emit_own_line_comments_in(span.start, span.end);
        }))
        .fmt(self);
        hard_line().fmt(self);
        token(close).fmt(self);
    }

    /// Emit comments dangling in an empty region (e.g. a function body with
    /// no statements), one per line. On the parsed path these are the
    /// unprinted comments before `end` that can reach it without crossing a
    /// token (see [`dangling_run`]); on the synthetic path they anchor to the
    /// region itself at `anchor` (a block span start). The caller provides
    /// surrounding line structure; only separators between multiple comments
    /// are emitted here.
    pub fn emit_dangling_comments(&mut self, anchor: u32, end: u32) {
        let texts: Vec<CompactString> = match &mut self.comments.store {
            Store::Sourced {
                entries,
                printed,
                source,
                ..
            } => {
                let count = dangling_run(&entries[*printed..], source, end);
                let taken = entries[*printed..*printed + count]
                    .iter()
                    .map(|entry| entry.text.clone())
                    .collect();
                *printed += count;
                taken
            }
            Store::Synthetic {
                entries, remaining, ..
            } => take_synthetic(entries, remaining, anchor, |_| true),
            Store::Empty => Vec::new(),
        };
        for (index, text) in texts.into_iter().enumerate() {
            if index > 0 {
                self.push(FormatElement::Line(LineMode::Hard));
            }
            self.push(FormatElement::Text(text));
        }
    }

    /// Flush comments left after the last statement (end of file).
    pub fn emit_remaining_comments(&mut self, has_statements_before: bool) {
        let texts: Vec<CompactString> = match &mut self.comments.store {
            Store::Sourced {
                entries, printed, ..
            } => {
                let taken = entries[*printed..]
                    .iter()
                    .map(|entry| entry.text.clone())
                    .collect();
                *printed = entries.len();
                taken
            }
            Store::Synthetic {
                entries, remaining, ..
            } => {
                *remaining = 0;
                std::mem::take(entries)
                    .into_values()
                    .flatten()
                    .map(|entry| synthetic_comment_text(&entry.text))
                    .collect()
            }
            Store::Empty => Vec::new(),
        };

        for (index, text) in texts.iter().enumerate() {
            if index > 0 || has_statements_before {
                self.push(FormatElement::Line(LineMode::Hard));
            }
            self.push(FormatElement::Text(text.clone()));
        }
    }
}

/// How many of `pending` an empty region ending at `end` may print: all the
/// comments before `end`, or none of them.
///
/// The region has no tokens of its own, so its comments print at its closing
/// keyword. Only a run separated from that keyword - and from each other - by
/// nothing but whitespace and `;` may move there; a comment left inside the
/// parameter list of an empty function would have to cross the `)`. An
/// unmovable run stays pending, and the statement holding it falls back to
/// verbatim.
fn dangling_run(pending: &[SourcedEntry], source: &str, end: u32) -> usize {
    let count = pending
        .iter()
        .take_while(|entry| entry.span_start < end)
        .count();
    let followers = pending[..count]
        .iter()
        .skip(1)
        .map(|entry| entry.span_start)
        .chain(std::iter::once(end));
    let is_movable = pending[..count]
        .iter()
        .zip(followers)
        .all(|(entry, next_start)| {
            next_start >= entry.span_end
                && is_code_free(&source[entry.span_end as usize..next_start as usize])
        });
    if is_movable { count } else { 0 }
}

/// Whether a stretch of source carries no token, so a comment may cross it
/// without landing between a different pair of tokens. Statement separators
/// count as absent: the formatter emits them at its own discretion.
fn is_code_free(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_whitespace() || byte == b';')
}

/// Drain the entries at `anchor` matching `filter`, preserving insertion
/// order, and keep the store's remaining-count in sync.
fn take_synthetic(
    entries: &mut BTreeMap<u32, Vec<SyntheticComment>>,
    remaining: &mut usize,
    anchor: u32,
    filter: impl Fn(&SyntheticComment) -> bool,
) -> Vec<CompactString> {
    let mut taken = Vec::new();
    if let Some(list) = entries.get_mut(&anchor) {
        list.retain(|entry| {
            if filter(entry) {
                taken.push(synthetic_comment_text(&entry.text));
                false
            } else {
                true
            }
        });
        if list.is_empty() {
            entries.remove(&anchor);
        }
    }
    *remaining -= taken.len();
    taken
}

/// Whether every comment of `original` survives in `formatted` with the same
/// text, between the same two tokens.
///
/// The two texts parse to the same AST but not to the same token stream:
/// `;` and `,` are separators the formatter writes at its own discretion (a
/// broken table gains a trailing comma, a `;` field separator becomes one),
/// and grouping parens come and go (`f"s"` gains them under
/// `call_parentheses = "always"`, a condition loses them). Separators are
/// dropped outright - a claimed list emits its comments as line suffixes, so
/// they always render after the separator whatever the source did. Parens are
/// aligned instead of dropped: a pair only one side has is skipped, and one
/// both sides keep is a real boundary, which is what catches a comment pulled
/// through a `(` that the emitters had no business moving.
pub(crate) fn comments_kept_in_place(
    original: &str,
    formatted: &str,
    version: luck_token::LuaVersion,
) -> bool {
    let before = luck_lexer::lex(original, version);
    let after = luck_lexer::lex(formatted, version);
    let before_tokens = significant_tokens(&before.tokens);
    let after_tokens = significant_tokens(&after.tokens);
    let (before_aligned, after_aligned) = align_tokens(&before_tokens, &after_tokens);
    comment_positions(original, &before_tokens, &before.comments, &before_aligned)
        == comment_positions(formatted, &after_tokens, &after.comments, &after_aligned)
}

/// The tokens a comment can be positioned against: everything but the
/// separators the formatter owns and the end marker.
fn significant_tokens(tokens: &[luck_token::Token]) -> Vec<&luck_token::Token> {
    use luck_token::TokenKind;
    tokens
        .iter()
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Semicolon | TokenKind::Comma | TokenKind::Eof
            )
        })
        .collect()
}

/// Mark the tokens present in both streams. The two differ only where the
/// formatter added or dropped a paren, so a lockstep walk that skips an
/// unmatched paren on either side realigns immediately; a genuine divergence
/// leaves the remaining tokens unaligned, which reads as a comment mismatch.
fn align_tokens(
    left: &[&luck_token::Token],
    right: &[&luck_token::Token],
) -> (Vec<bool>, Vec<bool>) {
    use luck_token::TokenKind;
    let is_paren = |token: &luck_token::Token| {
        matches!(token.kind, TokenKind::LeftParen | TokenKind::RightParen)
    };

    let mut left_aligned = vec![false; left.len()];
    let mut right_aligned = vec![false; right.len()];
    let (mut index, mut other) = (0, 0);
    while index < left.len() && other < right.len() {
        if std::mem::discriminant(&left[index].kind) == std::mem::discriminant(&right[other].kind) {
            left_aligned[index] = true;
            right_aligned[other] = true;
            index += 1;
            other += 1;
        } else if is_paren(left[index]) {
            index += 1;
        } else if is_paren(right[other]) {
            other += 1;
        } else {
            break;
        }
    }
    (left_aligned, right_aligned)
}

/// Each comment's text, keyed by how many aligned tokens precede it.
fn comment_positions<'a>(
    source: &'a str,
    tokens: &[&luck_token::Token],
    comments: &[Comment],
    aligned: &[bool],
) -> Vec<(usize, Cow<'a, str>)> {
    let mut next_token = 0;
    let mut preceding = 0;
    comments
        .iter()
        .map(|comment| {
            while let Some(token) = tokens.get(next_token) {
                if token.span.end > comment.span.start {
                    break;
                }
                preceding += usize::from(aligned[next_token]);
                next_token += 1;
            }
            let text = source[comment.span.start as usize..comment.span.end as usize].trim_end();
            let text = if text.contains('\r') {
                Cow::Owned(text.replace("\r\n", "\n"))
            } else {
                Cow::Borrowed(text)
            };
            (preceding, text)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use luck_token::Span;

    use super::*;

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
            preceded_by_newline: false,
            followed_by_newline: false,
        }
    }

    #[test]
    fn placement_check_sees_a_relocation() {
        let version = luck_token::LuaVersion::Lua54;
        let before = "if a --[[ condition ]] then run() end";
        assert!(!comments_kept_in_place(
            before,
            "if a then run() end --[[ condition ]]",
            version
        ));
        assert!(!comments_kept_in_place(
            before,
            "if a then run() end",
            version
        ));
        // Parens the formatter dropped and separators it wrote are not
        // boundaries the comment can be measured against.
        assert!(comments_kept_in_place(
            before,
            "if (a) --[[ condition ]] then\n run();\nend",
            version
        ));
    }

    #[test]
    fn placement_check_sees_a_comment_pulled_through_a_kept_paren() {
        // Both texts keep the call's parens, so moving the comment inside
        // them puts it between a different pair of tokens.
        assert!(!comments_kept_in_place(
            "render --[[ why ]] (a, b)",
            "render(\n\t--[[ why ]]\n\ta,\n\tb\n)",
            luck_token::LuaVersion::Lua54
        ));
    }

    #[test]
    fn placement_check_allows_the_parens_the_formatter_owns() {
        // `call_parentheses = "always"` wraps a bare table argument; the
        // comment inside it has not moved relative to any kept token.
        assert!(comments_kept_in_place(
            "render{a --[[ why ]]}",
            "render({ a --[[ why ]] })",
            luck_token::LuaVersion::Lua54
        ));
    }

    #[test]
    fn sourced_leading_taken_once() {
        let source = "-- leading\nlocal x = 1";
        let comments = vec![make_comment(
            0,
            10,
            11,
            CommentKind::Line,
            CommentPosition::Leading,
        )];
        let mut formatter = Formatter::with_context(
            crate::FormatOptions::default(),
            Comments::from_source(&comments, source, LuaVersion::Lua54),
        );

        assert!(!formatter.emit_leading_comments(11));
        let first_len = formatter.elements().len();
        assert!(first_len > 0);
        formatter.emit_leading_comments(11);
        assert_eq!(formatter.elements().len(), first_len);
    }

    #[test]
    fn ignore_directive_detected() {
        let source = "-- luck: ignore\nlocal x = 1";
        let comments = vec![make_comment(
            0,
            15,
            16,
            CommentKind::Line,
            CommentPosition::Leading,
        )];
        let mut formatter = Formatter::with_context(
            crate::FormatOptions::default(),
            Comments::from_source(&comments, source, LuaVersion::Lua54),
        );
        assert!(formatter.emit_leading_comments(16));
    }

    #[test]
    fn synthetic_comments_attach_by_anchor() {
        let synthetic = vec![
            SyntheticComment {
                attached_to: 5,
                text: "upvalue u0".into(),
                is_leading: true,
            },
            SyntheticComment {
                attached_to: 5,
                text: "-- explicit marker".into(),
                is_leading: false,
            },
        ];
        let mut formatter = Formatter::with_context(
            crate::FormatOptions::default(),
            Comments::synthetic(synthetic),
        );

        assert!(!formatter.emit_leading_comments(5));
        let has_prefixed = formatter
            .elements()
            .iter()
            .any(|element| matches!(element, FormatElement::Text(text) if text == "-- upvalue u0"));
        assert!(has_prefixed, "marker prefix added to bare text");

        formatter.emit_trailing_comments(5, 0);
        let has_suffix = formatter
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
            is_leading: true,
        }];
        let comments = Comments::synthetic(synthetic);
        assert!(comments.has_dangling_comments(9, 0));
        assert!(!comments.has_dangling_comments(8, 0));

        let mut formatter = Formatter::with_context(crate::FormatOptions::default(), comments);
        formatter.emit_dangling_comments(9, 0);
        let emitted = formatter.elements().iter().any(
            |element| matches!(element, FormatElement::Text(text) if text == "-- unreachable"),
        );
        assert!(emitted);
        assert!(!formatter.comments.has_dangling_comments(9, 0));
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
                is_leading: true,
            },
            SyntheticComment {
                attached_to: 10,
                text: "first".into(),
                is_leading: true,
            },
        ];
        let mut formatter = Formatter::with_context(
            crate::FormatOptions::default(),
            Comments::synthetic(synthetic),
        );
        formatter.emit_remaining_comments(false);
        let texts: Vec<&str> = formatter
            .elements()
            .iter()
            .filter_map(|element| match element {
                FormatElement::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["-- first", "-- second"]);
    }

    #[test]
    fn format_disabled_ranges() {
        let source = "-- luck: format off\nlocal x=1\n-- luck: format on\nlocal y = 2";
        let comments = vec![
            make_comment(0, 19, 20, CommentKind::Line, CommentPosition::Leading),
            make_comment(30, 49, 50, CommentKind::Line, CommentPosition::Leading),
        ];
        let store = Comments::from_source(&comments, source, LuaVersion::Lua54);
        assert!(store.is_format_disabled_at(25));
        assert!(!store.is_format_disabled_at(55));
    }
}
