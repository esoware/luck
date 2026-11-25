//! The document is one flat element stream, with structure marked by paired
//! start and end elements instead of nesting.
//!
//! Printing and measuring walk plain slices, and rolling back to a checkpoint
//! is a truncation. An element borrows its text from the tree, the source, the
//! arena, or the binary, so building the stream copies none. Text is bytes, as
//! the source is, so a comment or a string that isn't UTF-8 prints as written.
//!
//! The printer takes the stream as it's written, at each line break between
//! two statements that leaves nothing it has to see whole open, so the stream
//! holds a statement's elements or so at a time, and its buffer stays small
//! and in cache.
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
use luck_token::{Span, TokenKind};
use memchr::memchr2;
use unicode_width::UnicodeWidthStr as _;

use crate::FormatOptions;
use crate::comments::{Comments, CommentsCheckpoint};
use crate::printer::{Layout, LayoutCheckpoint, Printer, StreamWidth};

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
        // in most editors.
        let is_one_ascii_line = if text.len() <= 64 {
            is_one_ascii_line(text)
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

/// Whether `text` is ASCII with no LF or CR in it, read eight bytes at a time,
/// the last eight overlapping the word before them.
fn is_one_ascii_line(text: &[u8]) -> bool {
    const LOW_BITS: u64 = u64::from_ne_bytes([0x01; 8]);
    const HIGH_BITS: u64 = u64::from_ne_bytes([0x80; 8]);
    // In a word of ASCII, only a zero byte borrows into its own high bit when
    // every byte drops by one, and a borrow it passes on can only mark a byte
    // after it, so the test is exact for whether one is zero.
    let has_zero_byte = |word: u64| word.wrapping_sub(LOW_BITS) & !word & HIGH_BITS != 0;
    let is_one_line = |word: u64| {
        word & HIGH_BITS == 0
            && !has_zero_byte(word ^ u64::from_ne_bytes([b'\n'; 8]))
            && !has_zero_byte(word ^ u64::from_ne_bytes([b'\r'; 8]))
    };
    let Some(last) = text.last_chunk::<8>() else {
        return text.iter().all(|&b| b.is_ascii() && b != b'\n' && b != b'\r');
    };
    let (words, _) = text.as_chunks::<8>();
    words.iter().chain([last]).all(|word| is_one_line(u64::from_ne_bytes(*word)))
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

/// How much a formatter is about to write, so its buffers seldom grow.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Room {
    /// The bytes it prints.
    pub(crate) output: usize,
    /// The elements the stream holds at most before the printer takes them.
    pub(crate) elements: usize,
}

/// Restoring rewinds the element stream and its width, the layout, and the
/// comment cursor but not the group counter, so restoring hands out no id
/// twice. Only a group the printer has left gives its id up.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Checkpoint {
    element_count: usize,
    width: StreamWidth,
    layout: LayoutCheckpoint,
    comments: CommentsCheckpoint,
}

#[derive(Debug)]
pub(crate) struct Formatter<'ast> {
    /// The elements written since the printer last took them.
    elements: List<'ast, FormatElement<'ast>>,
    /// How many elements came before `elements`, so a position counts every
    /// element of the stream.
    printed_count: usize,
    printer: Printer<'ast>,
    /// How many regions the printer can't take part of are open: a fill, a
    /// conditional, a speculative write, or a run of requires to sort.
    holds: u32,
    width: StreamWidth,
    /// No space joins an element of `elements` before this one, which starts a
    /// run of elements an emitter moves or takes back as a unit.
    space_fence: usize,
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
    /// Whether a block printed here sorts its runs of requires. It starts as
    /// the `sort_requires` option has it, and a block inherits it from the
    /// one around it.
    sorts_requires: bool,
    /// Where text the formatter rewrites is built before it moves to the
    /// arena, so every rewrite shares one heap buffer.
    scratch: Vec<u8>,
}

impl<'ast> Formatter<'ast> {
    /// `groupings` are the parentheses the user of a parse wrote, and every
    /// line printed starts with `base_indent`.
    pub(crate) fn new(
        allocator: &'ast Allocator,
        options: FormatOptions,
        comments: Comments<'ast>,
        groupings: Option<&'ast Groupings>,
        base_indent: &'ast [u8],
        room: Room,
    ) -> Self {
        // About one element in eleven starts a group.
        let layout = Layout::with_capacity_in(room.elements.div_ceil(8), allocator);
        Self {
            elements: List::with_capacity_in(room.elements, allocator),
            printed_count: 0,
            printer: Printer::new(&options, base_indent, room.output, layout),
            holds: 0,
            width: StreamWidth::default(),
            space_fence: 0,
            options,
            allocator,
            comments,
            groupings,
            grouping_cursor: Cell::new(0),
            enclosing_end: u32::MAX,
            sorts_requires: options.sort_requires,
            scratch: Vec::new(),
        }
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

    /// Where the first `closer` at or after `from` is, for the comments
    /// before it, which the node being written holds none of when none is
    /// pending before its end. Any offset in the node places them the same
    /// then, so the tokens are searched only otherwise.
    pub(crate) fn closer_offset(&self, from: u32, closer: TokenKind) -> u32 {
        if self.comments.has_pending_before(self.enclosing_end) {
            self.comments.closer_offset(from, closer)
        } else {
            from
        }
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
    /// writes from here. No space written later joins an element before it,
    /// so that run can move or be taken back as a unit.
    pub(crate) fn mark(&mut self) -> usize {
        self.space_fence = self.elements.len();
        self.printed_count + self.space_fence
    }

    /// Where the next element goes in the stream.
    fn position(&self) -> usize {
        self.printed_count + self.elements.len()
    }

    /// Hands the elements written to the printer, right after a forced break,
    /// which ends every measurement, so the printer lays them out as it would
    /// with the rest of the stream after them. It waits while a group the
    /// printer would have to measure is open, or anything it must see whole.
    pub(crate) fn print_written(&mut self) {
        if self.holds > 0 || !self.printer.layout.is_settled() {
            return;
        }
        self.printer.print(&self.elements);
        self.printed_count += self.elements.len();
        self.elements.truncate(0);
        self.space_fence = 0;
        self.printer.layout.release_printed();
    }

    /// Keeps the printer from taking what's written until
    /// [`Formatter::release`], as a run of requires to sort needs.
    pub(crate) const fn hold(&mut self) {
        self.holds += 1;
    }

    pub(crate) const fn release(&mut self) {
        self.holds -= 1;
    }

    /// Prints the statements written to `slices`, element ranges in stream
    /// order, in `order`, which indexes `slices`. What lies between two of
    /// them stays in place. The printer has taken none of them yet, since a
    /// run of requires holds them back.
    pub(crate) fn reorder(&mut self, slices: &[Range<usize>], order: &[usize]) {
        let (Some(first), Some(last)) = (slices.first(), slices.last()) else {
            return;
        };
        let base = self.printed_count;
        let written = self.elements[first.start - base..last.end - base].to_vec();
        let mut target = first.start;
        let mut gap_start = first.start;
        for (slice, &i) in slices.iter().zip(order) {
            let gap = gap_start..slice.start;
            gap_start = slice.end;
            for range in [gap, slices[i].clone()] {
                let source = &written[range.start - first.start..range.end - first.start];
                let end = target + source.len();
                self.elements[target - base..end - base].copy_from_slice(source);
                target = end;
            }
        }
    }

    pub(crate) const fn options(&self) -> &FormatOptions {
        &self.options
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
    pub(crate) fn node(
        &mut self,
        span: impl FnOnce(&Self) -> Span,
        content: impl FnOnce(&mut Self),
    ) {
        self.write_node(span, true, content);
    }

    /// [`Formatter::node`] for a node whose `content` writes its leading
    /// comments itself, after the line break it may open with.
    #[inline]
    pub(crate) fn node_placing_leading_comments(
        &mut self,
        span: impl FnOnce(&Self) -> Span,
        content: impl FnOnce(&mut Self),
    ) {
        self.write_node(span, false, content);
    }

    /// `content` is written in one place only, since it's inlined into every
    /// node's emitter.
    #[inline]
    fn write_node(
        &mut self,
        span_of: impl FnOnce(&Self) -> Span,
        should_lead: bool,
        content: impl FnOnce(&mut Self),
    ) {
        let bounds = self.comments.has_pending_before(self.enclosing_end).then(|| {
            let span = span_of(self);
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
        self.printer.layout.new_group(self.allocator)
    }

    #[inline]
    fn push(&mut self, element: FormatElement<'ast>) {
        self.elements.push(element, self.allocator);
    }

    /// Text whose break, if it spans lines, the caller has already forced.
    #[inline]
    fn push_text(&mut self, text: &'ast [u8], width: TextWidth) {
        self.width.bytes += text.len();
        self.width.columns += width.columns();
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
        let width = TextWidth::of(text);
        if width.is_multiline() {
            self.printer.layout.force_break(self.position());
        }
        self.push_text(text, width);
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
        self.node(|_| name.span, |f| f.ascii(name.text.as_bytes()));
    }

    pub(crate) fn verbatim(&mut self, text: &'ast [u8]) {
        let width = TextWidth::of(text);
        self.width.bytes += text.len();
        self.width.columns += width.columns();
        if width.is_multiline() {
            self.printer.layout.force_break(self.position());
        }
        self.push(FormatElement::Verbatim(text, width));
    }

    /// See [`FormatElement::LineSuffix`]. A break in one forces nothing.
    pub(crate) fn line_suffix(&mut self, text: &'ast [u8], is_measured: bool) {
        self.width.bytes += text.len() + 1;
        self.printer.layout.line_suffix();
        self.push(FormatElement::LineSuffix(text, TextWidth::of(text), is_measured));
    }

    #[inline]
    pub(crate) fn space(&mut self) {
        self.width.bytes += 1;
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
        self.width.bytes += 1;
        self.printer.layout.line(self.position(), mode);
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
        self.printer.layout.force_break(self.position());
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
        self.printer.layout.open(id, self.position(), self.width, false);
        content(self);
        self.printer.layout.close(self.position(), self.width);
        self.push(FormatElement::EndGroup);
    }

    /// See [`FormatElement::StartHuggedGroup`].
    #[inline]
    pub(crate) fn hugged_group_with_id(&mut self, id: GroupId, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartHuggedGroup(id));
        self.printer.layout.open(id, self.position(), self.width, true);
        content(self);
        self.printer.layout.close(self.position(), self.width);
        self.push(FormatElement::EndGroup);
    }

    /// See [`FormatElement::StartNextLineGroup`].
    #[inline]
    pub(crate) fn next_line_group(&mut self, content: impl FnOnce(&mut Self)) {
        let id = self.group_id();
        self.push(FormatElement::StartNextLineGroup(id));
        self.printer.layout.open(id, self.position(), self.width, false);
        content(self);
        self.printer.layout.close(self.position(), self.width);
        self.push(FormatElement::EndNextLineGroup);
    }

    /// The group must start before this element, as for `if_group_breaks`.
    #[inline]
    pub(crate) fn indent_if_group_breaks(&mut self, id: GroupId, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartIndentIfGroupBreaks(id));
        content(self);
        self.push(FormatElement::EndIndentIfGroupBreaks(id));
    }

    /// The group must start before this element (enclosing, or an earlier
    /// sibling with no statement break between them, at which the printer
    /// can forget a group it has left). No flat measurement is sure to count
    /// the content's columns.
    #[inline]
    pub(crate) fn if_group_breaks(&mut self, id: GroupId, content: impl FnOnce(&mut Self)) {
        self.push(FormatElement::StartIfGroupBreaks(id));
        self.hold();
        let columns = self.width.columns;
        content(self);
        self.width.columns = columns;
        self.release();
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
        self.hold();
        content(self);
        self.release();
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

    /// Holds what's written from here back from the printer until the
    /// checkpoint is restored or kept.
    pub(crate) fn checkpoint(&mut self) -> Checkpoint {
        let element_count = self.mark();
        self.hold();
        Checkpoint {
            element_count,
            width: self.width,
            layout: self.printer.layout.checkpoint(element_count),
            comments: self.comments.checkpoint(),
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: Checkpoint) {
        self.elements.truncate(checkpoint.element_count - self.printed_count);
        self.width = checkpoint.width;
        self.printer.layout.restore(checkpoint.layout);
        self.comments.restore(checkpoint.comments);
        self.release();
    }

    /// Keeps what's been written since `checkpoint`.
    pub(crate) const fn keep(&mut self, _checkpoint: Checkpoint) {
        self.release();
    }

    /// Prints what's left of the stream.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        self.printer.print(&self.elements);
        self.printer.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_at_a_time_finds_what_a_byte_at_a_time_does() {
        // Every length up to three words, each with a byte at each position
        // that stops the scan or sits next to one that does, among fillers
        // that hold zero bytes, whose borrow is the scan's one subtlety.
        for length in 0..=24 {
            for position in 0..length {
                for stop in [b'\n', b'\r', 0x80, 0xFF, 0x00, 0x01, b'\t', b'\x0B', b'\x0E'] {
                    for filler in [b'a', 0x00, 0x01, b'\x0B'] {
                        let mut text = vec![filler; length];
                        text[position] = stop;
                        let expected =
                            text.iter().all(|&b| b.is_ascii() && b != b'\n' && b != b'\r');
                        assert_eq!(is_one_ascii_line(&text), expected, "{text:?}");
                    }
                }
            }
        }
    }
}
