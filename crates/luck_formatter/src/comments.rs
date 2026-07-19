//! Comment interleaving for both formatter input modes.
//!
//! Parsed source carries comments as a sorted `Vec<Comment>` keyed by byte
//! offsets (`attached_to`); a synthetic AST (decompiler output) carries
//! `SyntheticComment`s keyed by the anchor node's span start. Both resolve
//! to owned text at construction, so emission never slices source.
//!
//! The sourced store is a cursor over the sorted array (same model the old
//! formatter used); the synthetic store is anchor-keyed removal, because
//! synthesis order does not guarantee document order.

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
        /// Anchor-keyed; drained as anchors are formatted.
        entries: Vec<SyntheticComment>,
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
        Self {
            store: Store::Synthetic { entries: comments },
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

    /// Whether an unprinted comment starts before `end` - used to detect
    /// dangling comments in otherwise-empty bodies.
    pub fn has_pending_comments_before(&self, end: u32) -> bool {
        match &self.store {
            Store::Sourced {
                entries, printed, ..
            } => entries
                .get(*printed)
                .is_some_and(|entry| entry.span_start < end),
            Store::Synthetic { .. } | Store::Empty => false,
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
            Store::Synthetic { entries } => entries.len(),
            Store::Empty => 0,
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: usize) {
        match &mut self.store {
            Store::Sourced { printed, .. } => *printed = checkpoint,
            Store::Synthetic { entries } => {
                debug_assert!(
                    entries.len() == checkpoint,
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
            Store::Synthetic { entries } => {
                let mut taken = Vec::new();
                entries.retain(|entry| {
                    if entry.attached_to == anchor && entry.is_leading {
                        taken.push(synthetic_comment_text(&entry.text));
                        false
                    } else {
                        true
                    }
                });
                taken
            }
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
            Store::Synthetic { entries } => {
                let mut placements = Vec::new();
                entries.retain(|entry| {
                    if entry.attached_to == anchor && !entry.is_leading {
                        placements.push(Placement::Suffix(synthetic_comment_text(&entry.text)));
                        false
                    } else {
                        true
                    }
                });
                placements
            }
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
    /// no statements), one per line. The caller provides surrounding line
    /// structure; only separators between multiple comments are emitted here.
    pub fn emit_dangling_comments(&mut self, end: u32) {
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
            // Synthetic comments anchor to statements; an empty region has none
            Store::Synthetic { .. } | Store::Empty => Vec::new(),
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
            Store::Synthetic { entries } => entries
                .drain(..)
                .map(|entry| synthetic_comment_text(&entry.text))
                .collect(),
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
