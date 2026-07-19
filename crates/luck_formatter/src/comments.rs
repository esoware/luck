//! Comment interleaving for both formatter input modes.
//!
//! Parsed source carries comments as a sorted `Vec<Comment>` keyed by byte
//! offsets (`attached_to`); a synthetic AST (decompiler output) carries
//! `SyntheticComment`s keyed by the anchor node's span start. Both resolve
//! to owned text at construction, so emission never slices source.
//!
//! The sourced store is a cursor over the sorted array (same model the old
//! formatter used). The synthetic store is a map keyed by anchor, because
//! synthesis order does not guarantee document order and a decompiler may
//! attach comments to every statement - per-anchor lookup has to stay cheap
//! at that density. The synthetic store additionally carries the set of
//! anchors that want a blank line before them ([`Comments::with_blank_before`]),
//! the source-less replacement for blank-line preservation.

use std::collections::{BTreeMap, HashSet};

use compact_str::CompactString;
use luck_ast::synth::SyntheticComment;
use luck_token::comment::{Comment, CommentKind, CommentPosition};

use crate::ir::{FormatElement, Formatter, LineMode, Tag};

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

    pub fn from_source(comments: &[Comment], source: &str) -> Self {
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
            } else if trimmed.starts_with("luck: format on") {
                if let Some(start) = off_start.take() {
                    disabled_ranges.push((start, entry.span_start));
                }
            }
        }
        if let Some(start) = off_start {
            disabled_ranges.push((start, source.len() as u32));
        }

        Self {
            store: Store::Sourced {
                entries,
                source: source.to_string(),
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
    /// `anchors` - the source-less way to separate logical regions (basic
    /// blocks, decompiled protos). Only meaningful for synthetic input; on
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
    /// path any unprinted comment before `end`, on the synthetic path any
    /// comment anchored to the region itself at `anchor` (a block span start).
    pub fn has_dangling_comments(&self, anchor: u32, end: u32) -> bool {
        match &self.store {
            Store::Sourced {
                entries, printed, ..
            } => entries
                .get(*printed)
                .is_some_and(|entry| entry.span_start < end),
            Store::Synthetic { entries, .. } => entries.contains_key(&anchor),
            Store::Empty => false,
        }
    }

    /// Mark every comment starting before `end` as printed without emitting
    /// it - used when a statement is emitted verbatim from source, where the
    /// slice already contains its inner comments.
    pub(crate) fn mark_printed_through(&mut self, end: u32) {
        if let Store::Sourced {
            entries, printed, ..
        } = &mut self.store
        {
            while let Some(entry) = entries.get(*printed) {
                if entry.span_start < end {
                    *printed += 1;
                } else {
                    break;
                }
            }
        }
    }

    pub(crate) fn checkpoint(&self) -> usize {
        match &self.store {
            Store::Sourced { printed, .. } => *printed,
            Store::Synthetic { remaining, .. } => *remaining,
            Store::Empty => 0,
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: usize) {
        match &mut self.store {
            Store::Sourced { printed, .. } => *printed = checkpoint,
            Store::Synthetic { remaining, .. } => {
                debug_assert!(
                    *remaining == checkpoint,
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

    /// Emit trailing comments for the statement ending at `stmt_end`, plus
    /// any comments that lived inside the statement's span which no emitter
    /// visited. `anchor` is the statement's span start (synthetic path).
    pub fn emit_trailing_comments(&mut self, anchor: u32, stmt_end: u32) {
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
                // cursor and relocate every later comment to EOF.
                let mut has_inner = false;
                while let Some(entry) = entries.get(*printed) {
                    if entry.span_start >= stmt_end {
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
                    let gap = &source[stmt_end as usize..entry.span_start as usize];
                    if gap.contains('\n') {
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
                }
            }
        }
    }

    /// Emit comments dangling in an empty region (e.g. a function body with
    /// no statements), one per line. On the parsed path these are the
    /// unprinted comments before `end`; on the synthetic path they anchor to
    /// the region itself at `anchor` (a block span start). The caller
    /// provides surrounding line structure; only separators between multiple
    /// comments are emitted here.
    pub fn emit_dangling_comments(&mut self, anchor: u32, end: u32) {
        let texts: Vec<CompactString> = match &mut self.comments.store {
            Store::Sourced {
                entries, printed, ..
            } => {
                let mut taken = Vec::new();
                while let Some(entry) = entries.get(*printed) {
                    if entry.span_start < end {
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
            Comments::from_source(&comments, source),
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
            Comments::from_source(&comments, source),
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
        let store = Comments::from_source(&comments, source);
        assert!(store.is_format_disabled_at(25));
        assert!(!store.is_format_disabled_at(55));
    }
}
