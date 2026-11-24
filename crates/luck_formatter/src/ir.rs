//! The document is one flat element stream, with structure marked by paired
//! start and end elements instead of nesting.
//!
//! Printing and measuring walk plain slices, and rolling back to a checkpoint
//! is a truncation. An element borrows its text from the tree, the source, the
//! arena, or the binary, so building the stream copies none. Text is bytes, as
//! the source is, so a comment or a string that isn't UTF-8 prints as written.
//!
//! Emitters write through `Formatter`'s methods. Only this module pushes raw
//! elements, and each region takes its content as a closure, which is what
//! keeps every start paired with its end.

use std::cell::Cell;
use std::mem;
use std::ops::Range;

use luck_allocator::{Allocator, List};
use luck_ast::Name;
use luck_parser::Groupings;
use luck_token::{LuaVersion, Span};
use memchr::memchr2;
use unicode_width::UnicodeWidthStr as _;

use crate::FormatOptions;
use crate::comments::{Comments, CommentsCheckpoint};
use crate::printer::{self, Layout, LayoutCheckpoint};

/// Lets conditional content follow the break decision of a group other than
/// the innermost one around it. Doubles as the group's index into the
/// printer's group table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GroupId(u32);

impl GroupId {
    pub(crate) fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("a document opens fewer groups than `u32::MAX`"))
    }

    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineMode {
    /// Nothing when flat, newline when expanded.
    Soft,
    /// Space when flat, newline when expanded.
    SoftOrSpace,
    /// Always a newline; forces every enclosing group to expand.
    Hard,
    /// Always a blank line (two newlines); forces expansion like `Hard`.
    Empty,
}

/// Every `Start*` has a matching `End*`, which the region methods guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatElement<'ast> {
    /// May contain newlines (long strings); the printer measures from the
    /// last line.
    Text(&'ast [u8], TextWidth),
    /// Source copied as written. Text ending in a line break leaves the next
    /// line to the printer's indent, and any other starts at its own column.
    Verbatim(&'ast [u8], TextWidth),
    /// A space and the text, held back to the end of its line, which is how a
    /// comment that ended its line in the source ends it in the output. The
    /// line ends at the next line break, or before the next text other than
    /// a separator the comment was written after, as in `a, -- c`.
    ///
    /// The flag says whether the text counts toward the line's width. A block
    /// comment's does, as it does with code after it, so a line break that
    /// comes to follow it changes no layout on the next format. A line
    /// comment's doesn't, since it always ends its line and may run past the
    /// width, as in Prettier.
    LineSuffix(&'ast [u8], TextWidth, bool),
    Space,
    Line(LineMode),
    /// Force every enclosing group to expand without printing anything.
    ExpandParent,
    /// Content that prints flat if it fits the remaining line width.
    StartGroup(GroupId),
    /// A group that may break while the group right around it stays flat, as
    /// a call's function or table argument does. Closed by `EndGroup`.
    StartHuggedGroup(GroupId),
    EndGroup,
    /// A group that doesn't fit flat where it starts tries the next line, one
    /// indent deeper, before it breaks in place.
    StartNextLineGroup(GroupId),
    EndNextLineGroup,
    StartIndent,
    EndIndent,
    /// Content indented only when the referenced group expanded.
    StartIndentIfGroupBreaks(GroupId),
    EndIndentIfGroupBreaks(GroupId),
    /// Content emitted only when the referenced group expanded.
    StartIfGroupBreaks(GroupId),
    EndIfGroupBreaks,
    /// Content that prints flat whatever the width, with every nested group
    /// flat too.
    StartFlat,
    EndFlat,
    /// Pack as many entries per line as fit. Each entry is wrapped in
    /// `StartEntry`/`EndEntry`, with a `Line` separator between entries.
    StartFill,
    EndFill,
    StartEntry,
    EndEntry,
}

/// Taken once, as the text enters the stream, because the printer measures a
/// text again for every layout it tries.
///
/// The top bit says a space follows the text, which saves the stream a
/// `Space` after one text in five. The next says the text spans lines, and
/// the rest are the columns of its first line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextWidth(u32);

impl TextWidth {
    const SPACE_AFTER: u32 = 1 << 31;
    const MULTILINE: u32 = 1 << 30;
    const COLUMNS: u32 = Self::MULTILINE - 1;

    fn of(text: &[u8]) -> Self {
        // Without a line break, every ASCII character is one column. A short
        // text, the usual kind, takes one pass to tell, and a long one goes
        // to the vectorized scans. A lone CR breaks the line too, as it does
        // in Lua 5.x and in most editors.
        let is_one_ascii_line = if text.len() <= 64 {
            text.iter().all(|&b| b.is_ascii() && b != b'\n' && b != b'\r')
        } else {
            text.is_ascii() && memchr2(b'\n', b'\r', text).is_none()
        };
        if is_one_ascii_line {
            return Self::of_columns(text.len());
        }
        let first_line_length = memchr2(b'\n', b'\r', text).unwrap_or(text.len());
        let width = Self::of_columns(columns(&text[..first_line_length]));
        if first_line_length == text.len() { width } else { Self(width.0 | Self::MULTILINE) }
    }

    /// Any width past the line's compares the same.
    fn of_columns(columns: usize) -> Self {
        Self(u32::try_from(columns).unwrap_or(Self::COLUMNS).min(Self::COLUMNS))
    }

    /// The columns of the text's first line.
    pub(crate) const fn columns(self) -> usize {
        (self.0 & Self::COLUMNS) as usize
    }

    pub(crate) const fn is_multiline(self) -> bool {
        self.0 & Self::MULTILINE != 0
    }

    /// Whether a space follows the text, which then prints as a `Space`
    /// after it would.
    pub(crate) const fn has_space_after(self) -> bool {
        self.0 & Self::SPACE_AFTER != 0
    }

    const fn with_space_after(self) -> Self {
        Self(self.0 | Self::SPACE_AFTER)
    }
}

/// How many columns an editor shows `line` in: a character of UTF-8 as wide
/// as Unicode says, and any other byte as one, as a one-byte encoding such as
/// Latin-1 shows it.
pub(crate) fn columns(line: &[u8]) -> usize {
    // Checking for UTF-8 is word-at-a-time, but walking chunks is a byte at a
    // time, and nearly every line is UTF-8.
    match str::from_utf8(line) {
        Ok(text) => text.width(),
        Err(_) => {
            line.utf8_chunks().map(|chunk| chunk.valid().width() + chunk.invalid().len()).sum()
        }
    }
}

/// Restoring rewinds the element stream, the layout, and the comment cursor
/// but not the group counter, so no id is ever handed out twice.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Checkpoint {
    element_count: usize,
    layout: LayoutCheckpoint,
    comments: CommentsCheckpoint,
}

#[derive(Debug)]
pub(crate) struct Formatter<'ast> {
    elements: List<'ast, FormatElement<'ast>>,
    layout: Layout<'ast>,
    /// How many bytes of text the stream holds, which the output takes at
    /// least.
    text_bytes: usize,
    /// No space joins an element before this one, which starts a run of
    /// elements an emitter moves or takes back as a unit.
    space_fence: usize,
    /// The chunk's dialect, which spells a literal the tree doesn't, and reads
    /// the source's line breaks.
    version: LuaVersion,
    options: FormatOptions,
    allocator: &'ast Allocator,
    comments: Comments<'ast>,
    /// The parentheses a parse leaves out of its tree, which a tree built in
    /// code has none of.
    groupings: Option<&'ast Groupings>,
    /// Where the last search of `groupings` stopped, which the next starts
    /// from.
    grouping_cursor: Cell<usize>,
    /// Where the node being written ends, past which no node inside it may
    /// take a comment as its trailing one: a comment after the last child
    /// trails the node itself, or one around it.
    enclosing_end: u32,
    /// Whether a block printed here sorts its runs of requires. A whole tree
    /// starts with `sort_requires`, a range starts with it off, and a block
    /// inherits it from the one around it.
    sorts_requires: bool,
    /// Where text the formatter rewrites is built before it moves to the
    /// arena, so every rewrite shares one heap buffer.
    scratch: Vec<u8>,
}

impl<'ast> Formatter<'ast> {
    pub(crate) fn new(
        allocator: &'ast Allocator,
        version: LuaVersion,
        options: FormatOptions,
        comments: Comments<'ast>,
    ) -> Self {
        // A token or a comment comes to about two elements, and seldom to
        // more than two and a half, however long its text, so this room
        // seldom has to grow. A tree built in code has neither and starts
        // empty.
        let capacity = (comments.token_and_comment_count() * 5).div_ceil(2);
        Self {
            elements: List::with_capacity_in(capacity, allocator),
            // About one element in eleven starts a group.
            layout: Layout::with_capacity_in(capacity.div_ceil(8), allocator),
            text_bytes: 0,
            space_fence: 0,
            version,
            options,
            allocator,
            comments,
            groupings: None,
            grouping_cursor: Cell::new(0),
            enclosing_end: u32::MAX,
            sorts_requires: false,
            scratch: Vec::new(),
        }
    }

    pub(crate) const fn sorting_requires(mut self) -> Self {
        self.sorts_requires = true;
        self
    }

    /// Formats a parse, whose `groupings` are the parentheses its user wrote.
    pub(crate) const fn with_groupings(mut self, groupings: &'ast Groupings) -> Self {
        self.groupings = Some(groupings);
        self
    }

    /// The parentheses the source grouped the expression at `expr` in,
    /// which the formatter asks about in source order.
    pub(crate) fn grouping_around(&self, expr: Span) -> Option<Span> {
        let mut cursor = self.grouping_cursor.get();
        let parens = self.groupings?.around_from(expr, &mut cursor);
        self.grouping_cursor.set(cursor);
        parens
    }

    /// Where the node being written ends.
    pub(crate) const fn enclosing_end(&self) -> u32 {
        self.enclosing_end
    }

    pub(crate) const fn sorts_requires(&self) -> bool {
        self.sorts_requires
    }

    /// Writes a function whose block is no statement's own, so no block in
    /// it sorts its requires.
    pub(crate) fn without_sorting_requires(&mut self, content: impl FnOnce(&mut Self)) {
        let sorts_requires = mem::replace(&mut self.sorts_requires, false);
        content(self);
        self.sorts_requires = sorts_requires;
    }

    /// Where the next element goes, which an emitter notes to find what it
    /// writes from here.
    pub(crate) fn element_count(&mut self) -> usize {
        self.space_fence = self.elements.len();
        self.space_fence
    }

    /// Prints the statements written to `slices`, element ranges in stream
    /// order, in `order`, which indexes `slices`. What lies between two of
    /// them stays in place.
    pub(crate) fn reorder(&mut self, slices: &[Range<usize>], order: &[usize]) {
        let (Some(first), Some(last)) = (slices.first(), slices.last()) else {
            return;
        };
        let written = self.elements[first.start..last.end].to_vec();
        let mut target = first.start;
        let mut gap_start = first.start;
        for (slice, &i) in slices.iter().zip(order) {
            let gap = gap_start..slice.start;
            gap_start = slice.end;
            for range in [gap, slices[i].clone()] {
                let source = &written[range.start - first.start..range.end - first.start];
                let end = target + source.len();
                self.elements[target..end].copy_from_slice(source);
                target = end;
            }
        }
    }

    pub(crate) const fn options(&self) -> &FormatOptions {
        &self.options
    }

    pub(crate) const fn version(&self) -> LuaVersion {
        self.version
    }

    pub(crate) const fn allocator(&self) -> &'ast Allocator {
        self.allocator
    }

    pub(crate) const fn comments(&self) -> &Comments<'ast> {
        &self.comments
    }

    pub(crate) const fn comments_mut(&mut self) -> &mut Comments<'ast> {
        &mut self.comments
    }

    /// Writes a node at `span`: its leading comments, `content`, and its
    /// trailing comments. Any comment inside it that no node inside takes is
    /// `content`'s to place.
    ///
    /// When the next comment starts past the node around this one, the node
    /// has none, nothing inside takes one either, and the cursor doesn't move
    /// while it's written, so the bound it would set for them can't matter.
    /// That's nearly every node, which then needs no span, so `span` is only
    /// worked out otherwise.
    #[inline]
    pub(crate) fn node(&mut self, span: impl FnOnce() -> Span, content: impl FnOnce(&mut Self)) {
        self.write_node(span, true, content);
    }

    /// [`Formatter::node`] for a node whose `content` writes its leading
    /// comments itself, after the line break it may open with.
    #[inline]
    pub(crate) fn node_placing_leading_comments(
        &mut self,
        span: impl FnOnce() -> Span,
        content: impl FnOnce(&mut Self),
    ) {
        self.write_node(span, false, content);
    }

    /// `content` is written in one place only, since it's inlined into every
    /// node's emitter.
    #[inline]
    fn write_node(
        &mut self,
        span_of: impl FnOnce() -> Span,
        should_lead: bool,
        content: impl FnOnce(&mut Self),
    ) {
        let bounds = self.comments.has_pending_before(self.enclosing_end).then(|| {
            let span = span_of();
            if should_lead {
                self.write_leading_comments(span.start);
            }
            (span.end, mem::replace(&mut self.enclosing_end, span.end))
        });
        content(self);
        if let Some((end, enclosing_end)) = bounds {
            self.enclosing_end = enclosing_end;
            self.write_trailing_comments(end, enclosing_end);
        }
    }

    /// Writes `content`, the inside of a node ending at `end` that places its
    /// own comments, as a statement does.
    #[inline]
    pub(crate) fn inside(&mut self, end: u32, content: impl FnOnce(&mut Self)) {
        let enclosing_end = mem::replace(&mut self.enclosing_end, end);
        content(self);
        self.enclosing_end = enclosing_end;
    }

    /// The scratch buffer, emptied, and the arena its text moves to.
    pub(crate) fn scratch(&mut self) -> (&mut Vec<u8>, &'ast Allocator) {
        self.scratch.clear();
        (&mut self.scratch, self.allocator)
    }

    pub(crate) fn group_id(&mut self) -> GroupId {
        self.layout.new_group(self.allocator)
    }

    #[inline]
    fn push(&mut self, element: FormatElement<'ast>) {
        self.elements.push(element, self.allocator);
    }

    #[inline]
    fn push_text(&mut self, text: &'ast [u8], width: TextWidth) {
        self.text_bytes += text.len();
        if width.is_multiline() {
            self.layout.force_break(self.elements.len());
        }
        self.push(FormatElement::Text(text, width));
    }

    /// ASCII with no line break, so as many columns wide as it is long.
    #[inline]
    pub(crate) fn token(&mut self, text: &'static str) {
        debug_assert!(text.is_ascii() && !text.contains('\n'), "a token is measured by its length");
        self.push_text(text.as_bytes(), TextWidth::of_columns(text.len()));
    }

    #[inline]
    pub(crate) fn text(&mut self, text: &'ast [u8]) {
        self.push_text(text, TextWidth::of(text));
    }

    /// An identifier or a numeral, which the lexer and `Synth` only ever make
    /// of ASCII on one line, so it's measured by its length too.
    #[inline]
    pub(crate) fn ascii(&mut self, text: &'ast [u8]) {
        self.push_text(text, TextWidth::of_columns(text.len()));
    }

    /// A name is a node of its own, which comments can lead and trail.
    #[inline]
    pub(crate) fn name(&mut self, name: Name<'ast>) {
        self.node(|| name.span, |f| f.ascii(name.text.as_bytes()));
    }

    pub(crate) fn verbatim(&mut self, text: &'ast [u8]) {
        let width = TextWidth::of(text);
        self.text_bytes += text.len();
        if width.is_multiline() {
            self.layout.force_break(self.elements.len());
        }
        self.push(FormatElement::Verbatim(text, width));
    }

    /// See [`FormatElement::LineSuffix`]. A break in one forces nothing.
    pub(crate) fn line_suffix(&mut self, text: &'ast [u8], is_measured: bool) {
        self.text_bytes += text.len() + 1;
        self.push(FormatElement::LineSuffix(text, TextWidth::of(text), is_measured));
    }

    #[inline]
    pub(crate) fn space(&mut self) {
        self.text_bytes += 1;
        if self.elements.len() > self.space_fence
            && let Some(FormatElement::Text(_, width)) = self.elements.last_mut()
        {
            *width = width.with_space_after();
            return;
        }
        self.push(FormatElement::Space);
    }

    #[inline]
    pub(crate) fn line(&mut self, mode: LineMode) {
        self.text_bytes += 1;
        self.layout.line(self.elements.len(), mode);
        self.push(FormatElement::Line(mode));
    }

    #[inline]
    pub(crate) fn soft_line(&mut self) {
        self.line(LineMode::Soft);
    }

    #[inline]
    pub(crate) fn soft_line_or_space(&mut self) {
        self.line(LineMode::SoftOrSpace);
    }

    #[inline]
    pub(crate) fn hard_line(&mut self) {
        self.line(LineMode::Hard);
    }

    #[inline]
    pub(crate) fn empty_line(&mut self) {
        self.line(LineMode::Empty);
    }

    /// A blank line when `is_blank`, and a plain one otherwise.
    #[inline]
    pub(crate) fn line_break(&mut self, is_blank: bool) {
        self.line(if is_blank { LineMode::Empty } else { LineMode::Hard });
    }

    pub(crate) fn expand_parent(&mut self) {
        self.layout.force_break(self.elements.len());
        self.push(FormatElement::ExpandParent);
    }

    #[inline]
    pub(crate) fn indent(&mut self, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartIndent);
        content(self);
        self.push(FormatElement::EndIndent);
    }

    #[inline]
    pub(crate) fn group(&mut self, content: impl FnOnce(&mut Self)) {
        let id = self.group_id();
        self.group_with_id(id, content);
    }

    /// A group with a caller-allocated id, so other elements can follow its
    /// break decision.
    #[inline]
    pub(crate) fn group_with_id(&mut self, id: GroupId, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartGroup(id));
        self.layout.open(id, self.elements.len(), false);
        content(self);
        self.layout.close();
        self.push(FormatElement::EndGroup);
    }

    /// See [`FormatElement::StartHuggedGroup`].
    #[inline]
    pub(crate) fn hugged_group_with_id(&mut self, id: GroupId, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartHuggedGroup(id));
        self.layout.open(id, self.elements.len(), true);
        content(self);
        self.layout.close();
        self.push(FormatElement::EndGroup);
    }

    /// See [`FormatElement::StartNextLineGroup`].
    #[inline]
    pub(crate) fn next_line_group(&mut self, content: impl FnOnce(&mut Self)) {
        let id = self.group_id();
        self.push(FormatElement::StartNextLineGroup(id));
        self.layout.open(id, self.elements.len(), false);
        content(self);
        self.layout.close();
        self.push(FormatElement::EndNextLineGroup);
    }

    /// The group must start before this element, as for `if_group_breaks`.
    #[inline]
    pub(crate) fn indent_if_group_breaks(&mut self, id: GroupId, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartIndentIfGroupBreaks(id));
        content(self);
        self.push(FormatElement::EndIndentIfGroupBreaks(id));
    }

    /// The group must start before this element (enclosing or earlier
    /// sibling).
    #[inline]
    pub(crate) fn if_group_breaks(&mut self, id: GroupId, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartIfGroupBreaks(id));
        content(self);
        self.push(FormatElement::EndIfGroupBreaks);
    }

    #[inline]
    pub(crate) fn flat(&mut self, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartFlat);
        content(self);
        self.push(FormatElement::EndFlat);
    }

    /// Packs entries onto lines greedily. `content` writes them with
    /// [`Formatter::fill_entry`].
    #[inline]
    pub(crate) fn fill(&mut self, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartFill);
        content(self);
        self.push(FormatElement::EndFill);
    }

    /// An entry with the line that parts it from the one before, which the
    /// first entry has none of. Entry boundaries are explicit so the printer
    /// can measure each entry as a unit.
    #[inline]
    pub(crate) fn fill_entry(
        &mut self,
        separator: Option<LineMode>,
        content: impl FnOnce(&mut Self),
    ) {
        if let Some(mode) = separator {
            self.line(mode);
        }
        self.push(FormatElement::StartEntry);
        content(self);
        self.push(FormatElement::EndEntry);
    }

    #[cfg(test)]
    pub(crate) fn elements(&self) -> &[FormatElement<'ast>] {
        &self.elements
    }

    pub(crate) fn checkpoint(&mut self) -> Checkpoint {
        let element_count = self.element_count();
        Checkpoint {
            element_count,
            layout: self.layout.checkpoint(element_count),
            comments: self.comments.checkpoint(),
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: Checkpoint) {
        self.elements.truncate(checkpoint.element_count);
        self.layout.restore(checkpoint.layout);
        self.comments.restore(checkpoint.comments);
    }

    pub(crate) fn print(self, base_indent: &[u8]) -> Vec<u8> {
        // Indentation comes on top of the text, and seldom reaches a quarter
        // of it.
        let capacity = self.text_bytes + self.text_bytes.div_ceil(4);
        printer::print(&self.elements, self.layout, &self.options, base_indent, capacity)
    }
}
