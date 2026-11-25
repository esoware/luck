//! Comment emission: each comment the store hands a node boundary goes out
//! where the source had it, on its line or on a line of its own.
//!
//! A parsed comment's text is sliced from the source as it's written, bytes
//! that need not be UTF-8, and only one whose line breaks need rewriting is
//! copied, to the arena.

use std::iter::once;

use luck_token::{Comment, CommentContent, CommentKind, CommentPosition, line_breaks};

use crate::comments::{ends_line, is_long_comment};
use crate::ir::{Formatter, LineMode};
use crate::lines::{has_blank_line, has_leading_edge_blank, starts_line};

impl<'ast> Formatter<'ast> {
    /// The text a parsed comment prints as, with its line breaks written as
    /// LF.
    fn comment_text(&mut self, comment: &Comment) -> &'ast [u8] {
        let Some(source) = self.comments().source_text() else {
            return b"";
        };
        let text = comment.span.text(source);
        if comment.kind == CommentKind::Shebang {
            // Loaders skip this line up to LF, so a CR in it is text, but one
            // the printer leaves at its end, past the spaces and tabs it
            // trims, would pair with the line break written after it.
            let length = text
                .iter()
                .rposition(|b| !matches!(b, b'\r' | b' ' | b'\t'))
                .map_or(0, |last| last + 1);
            return &text[..length];
        }
        // Only a block comment the lexer saw a line break in holds one.
        if comment.kind != CommentKind::MultiLineBlock || !text.contains(&b'\r') {
            return text;
        }
        let (normalized, allocator) = self.scratch();
        let mut cursor = 0;
        for line_break in line_breaks(text) {
            // A lone CR is a space, and one left before the LF would make a
            // CRLF of it.
            let line = &text[cursor..line_break.start];
            let length = line.iter().rposition(|&b| b != b'\r').map_or(0, |last| last + 1);
            normalized.extend_from_slice(&line[..length]);
            normalized.push(b'\n');
            cursor = line_break.end;
        }
        normalized.extend_from_slice(&text[cursor..]);
        allocator.alloc_slice_copy(normalized)
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
        let Some(source) = self.comments().source_text() else {
            return false;
        };
        let taken = self.comments_mut().take_before(start);
        let mut has_ignore = false;
        let followers = taken.iter().skip(1).map(|c| c.span.start).chain(once(start));
        for (comment, next_start) in taken.iter().zip(followers) {
            let is_line_ending = ends_line(comment);
            if is_line_ending && starts_line(source, comment.span.start) {
                self.hard_line();
            }
            has_ignore |= comment.content == CommentContent::FormatIgnore;
            let text = self.comment_text(comment);
            self.text(text);
            if is_line_ending {
                self.line_break(has_blank_line(source, comment.span.end, next_start));
            } else {
                self.space();
            }
        }
        has_ignore
    }

    /// Writes the leading comments of the operand `start` finds the start of
    /// ahead of the operator before it, which starts the operand's line when
    /// the chain breaks, when one of them ends its line. A comment that
    /// doesn't stays with the operand, after the operator.
    pub(crate) fn write_hoisted_comments(&mut self, start: impl FnOnce(&Self) -> u32) {
        if !self.comments().has_pending_before(self.enclosing_end()) {
            return;
        }
        let start = start(self);
        if self.comments().has_line_ending_comment_before(start) {
            self.write_leading_comments(start);
        }
    }

    /// Whether a comment leading the node `start` finds the start of ends its
    /// line. That node is inside the one being written, which holds no comment
    /// when none is pending before its end, so `start` is only worked out
    /// otherwise.
    pub(crate) fn has_line_ending_comment_before(&self, start: impl FnOnce(&Self) -> u32) -> bool {
        self.comments().has_pending_before(self.enclosing_end())
            && self.comments().has_line_ending_comment_before(start(self))
    }

    /// Writes the comments right after a node ending at `end`, on its line and
    /// before `enclosing_end`, where the node around it ends, as
    /// [`Comments::take_trailing`](crate::comments::Comments::take_trailing)
    /// takes them.
    ///
    /// Those that go to the end of the line break the groups around them.
    #[inline]
    pub(crate) fn write_trailing_comments(&mut self, end: u32, enclosing_end: u32) {
        if self.comments().has_pending_before(enclosing_end) {
            self.write_trailing_run(end, enclosing_end);
        }
    }

    fn write_trailing_run(&mut self, end: u32, enclosing_end: u32) {
        let (in_place, at_line_end) = self.comments_mut().take_trailing(end, enclosing_end);
        for comment in in_place {
            let text = self.comment_text(comment);
            self.space();
            self.text(text);
        }
        for comment in at_line_end {
            let text = self.comment_text(comment);
            self.line_suffix(text, comment.kind != CommentKind::Line);
        }
        if !at_line_end.is_empty() {
            self.expand_parent();
        }
    }

    /// Writes the comments before a container's closer at `closer`, after its
    /// last item. Each keeps the line it starts or the place on its line, and
    /// a line comment ends the line it's on.
    #[inline]
    pub(crate) fn write_closing_comments(&mut self, closer: u32) {
        if self.comments().has_pending_before(closer) {
            self.write_closing_run(closer, false);
        }
    }

    /// [`Formatter::write_closing_comments`] for a table, where a comment on
    /// a line of its own also keeps the blank line above it.
    #[inline]
    pub(crate) fn write_closing_comments_keeping_blank_lines(&mut self, closer: u32) {
        if self.comments().has_pending_before(closer) {
            self.write_closing_run(closer, true);
        }
    }

    fn write_closing_run(&mut self, closer: u32, keeps_blank_lines: bool) {
        let Some(source) = self.comments().source_text() else {
            return;
        };
        let mut has_ended_line = false;
        for comment in self.comments_mut().take_before(closer) {
            match (starts_line(source, comment.span.start), has_ended_line) {
                (true, _) => {
                    let is_blank =
                        keeps_blank_lines && has_leading_edge_blank(source, comment.span.start);
                    self.line_break(is_blank);
                }
                (false, false) => self.space(),
                // The line comment before it ended the line it was on.
                (false, true) => {}
            }
            let text = self.comment_text(comment);
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
        let taken =
            if comments.has_pending_before(closer) { comments.take_before(closer) } else { &[] };
        let Some(source) = self.comments().source_text().filter(|_| !taken.is_empty()) else {
            self.token(open);
            self.token(close);
            return;
        };
        self.group(|grouped| {
            grouped.token(open);
            grouped.indent(|f| {
                f.line(line);
                let mut previous: Option<&Comment> = None;
                for comment in taken {
                    if let Some(before) = previous {
                        if before.kind == CommentKind::Line
                            || starts_line(source, comment.span.start)
                        {
                            let gap_start = before.span.end;
                            f.line_break(has_blank_line(source, gap_start, comment.span.start));
                        } else {
                            f.space();
                        }
                    }
                    let text = f.comment_text(comment);
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
        let allocator = self.allocator();
        let lines = self.comments_mut().take_synthetic(anchor, allocator, |comment| {
            comment.position == CommentPosition::Leading
        });
        for text in lines {
            self.text(text.as_bytes());
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
        if self.comments().source_text().is_none() {
            let allocator = self.allocator();
            let texts = self.comments_mut().take_synthetic(anchor, allocator, |comment| {
                comment.position == CommentPosition::Trailing
            });
            for text in &texts {
                self.line_suffix(text.as_bytes(), is_long_comment(text));
            }
            if !texts.is_empty() {
                self.expand_parent();
            }
            return false;
        }
        let taken = self.comments_mut().take_stmt_trailing(stmt_end);
        if taken.is_empty() {
            return false;
        }
        for comment in taken {
            let text = self.comment_text(comment);
            self.line_suffix(text, comment.kind != CommentKind::Line);
        }
        self.expand_parent();
        taken.iter().any(|comment| comment.kind == CommentKind::MultiLineBlock)
    }

    /// Writes `comments` a line each with the line breaks between them, but
    /// none after the last.
    fn write_comment_run(&mut self, comments: &[Comment]) {
        let Some(source) = self.comments().source_text() else {
            return;
        };
        let mut previous_end = None;
        for comment in comments {
            if let Some(end) = previous_end {
                self.line_break(has_blank_line(source, end, comment.span.start));
            }
            previous_end = Some(comment.span.end);
            let text = self.comment_text(comment);
            self.text(text);
        }
    }

    pub(crate) fn write_shebang(&mut self) {
        for comment in self.comments_mut().take_shebang() {
            let text = self.comment_text(comment);
            self.text(text);
            self.hard_line();
        }
    }

    /// Write the comments in a block with no statements, whose span starts at
    /// `block_start`, one per line.
    ///
    /// On the parsed path that's the run
    /// [`Comments::take_dangling`](crate::comments::Comments::take_dangling)
    /// takes there; on the synthetic path, every comment anchored at
    /// `block_start`. The caller writes the line breaks around them.
    pub(crate) fn write_dangling_comments(&mut self, block_start: u32) {
        if self.comments().source_text().is_some() {
            let taken = self.comments_mut().take_dangling(block_start);
            self.write_comment_run(taken);
            return;
        }
        let allocator = self.allocator();
        let lines = self.comments_mut().take_synthetic(block_start, allocator, |_| true);
        for (i, text) in lines.into_iter().enumerate() {
            if i > 0 {
                self.hard_line();
            }
            self.text(text.as_bytes());
        }
    }

    /// Write the comments after a block's last statement, each on its own
    /// line and keeping the blank lines around them.
    ///
    /// That's the run
    /// [`Comments::take_dangling`](crate::comments::Comments::take_dangling)
    /// takes after `last_end`, so a comment only moves past a `;`. Parsed
    /// input only: a synthetic comment names its anchor.
    ///
    /// When that run would start inside the node being written, the block's
    /// closer lies between it and any comment pending past the node's end, so
    /// with none pending before that end, the run is empty.
    pub(crate) fn write_tail_comments(&mut self, last_end: u32) {
        let Some(source) = self.comments().source_text() else {
            return;
        };
        let gap_start =
            self.comments().last_printed_end().map_or(last_end, |end| end.max(last_end));
        let enclosing_end = self.enclosing_end();
        if gap_start <= enclosing_end && !self.comments().has_pending_before(enclosing_end) {
            return;
        }
        let taken = self.comments_mut().take_dangling(gap_start);
        let Some(first) = taken.first() else {
            return;
        };
        self.line_break(has_blank_line(source, gap_start, first.span.start));
        self.write_comment_run(taken);
    }

    pub(crate) fn write_remaining_comments(&mut self, has_stmts_before: bool) {
        let taken = self.comments_mut().take_remaining();
        debug_assert!(taken.is_empty(), "a comment with no place to go: {taken:?}");
        for (i, comment) in taken.iter().enumerate() {
            if i > 0 || has_stmts_before {
                self.hard_line();
            }
            let text = self.comment_text(comment);
            self.text(text);
        }
        let allocator = self.allocator();
        let lines = self.comments_mut().take_remaining_synthetic(allocator);
        for (i, text) in lines.into_iter().enumerate() {
            if i > 0 || has_stmts_before {
                self.hard_line();
            }
            self.text(text.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_token::Span;

    use super::*;
    use crate::FormatOptions;
    use crate::comments::{Comments, SyntheticComment};
    use crate::fixtures::parse;
    use crate::ir::{FormatElement, Room};

    fn formatter<'ast>(allocator: &'ast Allocator, comments: Comments<'ast>) -> Formatter<'ast> {
        Formatter::new(allocator, FormatOptions::default(), comments, None, b"", Room::default())
    }

    fn texts<'ast>(f: &Formatter<'ast>) -> Vec<&'ast str> {
        f.elements()
            .iter()
            .filter_map(|element| {
                if let FormatElement::Text(text, _) = element {
                    Some(str::from_utf8(text).unwrap())
                } else {
                    None
                }
            })
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
    fn a_leading_comment_is_written_once() {
        let allocator = Allocator::new();
        let source = "\
-- leading
local x = 1";
        let parsed = parse(&allocator, source);
        let mut f = formatter(&allocator, Comments::from_parse(&parsed));

        assert!(!f.write_stmt_leading_comments(11));
        assert_eq!(texts(&f), ["-- leading"]);
        f.write_stmt_leading_comments(11);
        assert_eq!(texts(&f), ["-- leading"]);
    }

    #[test]
    fn a_leading_ignore_directive_is_reported() {
        let allocator = Allocator::new();
        let source = "\
-- luck: ignore
local x = 1";
        let parsed = parse(&allocator, source);
        let mut f = formatter(&allocator, Comments::from_parse(&parsed));
        assert!(f.write_stmt_leading_comments(16));
    }

    #[test]
    fn line_breaks_in_a_comment_print_as_lf() {
        let allocator = Allocator::new();
        let parsed = parse(&allocator, "--[[a\r\nb]]");
        let mut f = formatter(&allocator, Comments::from_parse(&parsed));
        f.write_leading_comments(10);
        assert_eq!(texts(&f), ["--[[a\nb]]"]);
    }

    #[test]
    fn synthetic_comments_attach_by_anchor() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([
            synthetic(5, "upvalue u0", CommentPosition::Leading),
            synthetic(5, "-- explicit marker", CommentPosition::Trailing),
        ]);
        let mut f = formatter(&allocator, store);

        assert!(!f.write_stmt_leading_comments(5));
        assert_eq!(texts(&f), ["-- upvalue u0"], "marker prefix added to bare text");

        assert!(!f.write_stmt_trailing_comments(5, 0));
        let is_suffix = |element: &FormatElement<'_>| {
            matches!(element, FormatElement::LineSuffix(b"-- explicit marker", _, false))
        };
        assert!(f.elements().iter().any(is_suffix));
    }

    #[test]
    fn synthetic_comments_stay_off_the_nodes_inside_a_statement() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([synthetic(5, "kept", CommentPosition::Leading)]);
        let mut f = formatter(&allocator, store);
        f.node(|_| Span::new(5, 5), |_| {});
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

        let mut f = formatter(&allocator, store);
        f.write_dangling_comments(9);
        assert_eq!(texts(&f), ["-- unreachable"]);
        assert!(!f.comments().has_dangling_comments(9));
    }

    #[test]
    fn synthetic_remaining_flushes_in_anchor_order() {
        let allocator = Allocator::new();
        let store = Comments::synthetic([
            synthetic(20, "second", CommentPosition::Leading),
            synthetic(10, "first", CommentPosition::Leading),
        ]);
        let mut f = formatter(&allocator, store);
        f.write_remaining_comments(false);
        assert_eq!(texts(&f), ["-- first", "-- second"]);
    }
}
