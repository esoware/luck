//! The document is one flat element stream, with structure marked by paired
//! `Tag` start and end elements instead of nesting.
//!
//! Printing and measuring walk plain slices, and rolling back to a checkpoint is a truncation.
//!
//! Emitters write through the [`Format`] trait, the combinators in this
//! module, and `Formatter`'s `write_*` methods. Only this module pushes raw
//! elements, which is what keeps every tag paired.

use core::ops::Range;

use luck_ast::Block;
use luck_token::{CompactString, Span};
use rustc_hash::FxHashSet;
use unicode_width::UnicodeWidthStr as _;

use crate::comments::{Comments, CommentsCheckpoint};
use crate::{FormatOptions, printer};

/// Lets conditional content follow the break decision of a group other than
/// the innermost one around it. Doubles as the group's index into the
/// printer's mode table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GroupId(usize);

impl GroupId {
    pub(crate) const fn index(self) -> usize {
        self.0
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

/// Every `Start*` has a matching `End*`; the builder combinators guarantee pairing.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub(crate) enum Tag {
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
    /// Content deferred to just before the next line break (trailing comments).
    StartLineSuffix,
    EndLineSuffix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FormatElement {
    /// ASCII, so as many columns wide as it is long.
    Token(&'static str),
    /// May contain newlines (long strings); the printer measures from the last line.
    Text(CompactString, TextWidth),
    Verbatim(CompactString, TextWidth),
    Space,
    Line(LineMode),
    /// Force every enclosing group to expand without printing anything.
    ExpandParent,
    /// Break the line here if a line suffix is pending, so a trailing comment
    /// can't swallow what follows.
    LineSuffixBoundary,
    Tag(Tag),
}

/// Taken once, as the text enters the stream, because the printer measures a
/// text again for every layout it tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextWidth(u32);

impl TextWidth {
    const MULTILINE: Self = Self(u32::MAX);

    fn of(text: &str) -> Self {
        if text.contains('\n') {
            return Self::MULTILINE;
        }
        // Without a line break, `width` counts every ASCII character as one
        // column.
        let columns = if text.is_ascii() { text.len() } else { text.width() };
        // Any width past the line's compares the same, and `MULTILINE` takes
        // `u32::MAX`.
        Self(u32::try_from(columns).unwrap_or(u32::MAX).min(u32::MAX - 1))
    }

    /// `None` for text that spans lines.
    pub(crate) fn columns(self) -> Option<usize> {
        (self != Self::MULTILINE).then(|| usize::try_from(self.0).unwrap_or(usize::MAX))
    }
}

pub(crate) trait Format {
    fn fmt(&self, f: &mut Formatter);
}

impl<T: Format + ?Sized> Format for &T {
    fn fmt(&self, f: &mut Formatter) {
        (**self).fmt(f);
    }
}

impl<T: Format> Format for Option<T> {
    fn fmt(&self, f: &mut Formatter) {
        if let Some(value) = self {
            value.fmt(f);
        }
    }
}

macro_rules! tuple_format {
    ($($name:ident)+) => {
        impl<$($name: Format),+> Format for ($($name,)+) {
            fn fmt(&self, f: &mut Formatter) {
                #[expect(
                    non_snake_case,
                    reason = "each binding reuses its generic parameter's name"
                )]
                let ($($name,)+) = self;
                $($name.fmt(f);)+
            }
        }
    };
}
tuple_format!(First Second);
tuple_format!(First Second Third);
tuple_format!(First Second Third Fourth);
tuple_format!(First Second Third Fourth Fifth);
tuple_format!(First Second Third Fourth Fifth Sixth);
tuple_format!(First Second Third Fourth Fifth Sixth Seventh);
tuple_format!(First Second Third Fourth Fifth Sixth Seventh Eighth);
tuple_format!(First Second Third Fourth Fifth Sixth Seventh Eighth Ninth);
tuple_format!(First Second Third Fourth Fifth Sixth Seventh Eighth Ninth Tenth);

/// Restoring rewinds the element stream and the comment cursor but not the
/// group id counter, so no id is ever handed out twice.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Checkpoint {
    element_count: usize,
    comments: CommentsCheckpoint,
}

#[derive(Debug)]
pub(crate) struct Formatter {
    elements: Vec<FormatElement>,
    next_group_id: usize,
    options: FormatOptions,
    comments: Comments,
    /// The spans of the blocks whose runs of requires print sorted, which
    /// only [`crate::format()`] asks for.
    sorting_blocks: FxHashSet<Span>,
}

impl Formatter {
    pub(crate) fn new(options: FormatOptions, comments: Comments) -> Self {
        // Roughly one element per 8 source bytes; a mild undershoot that
        // still removes most realloc copies. Synthetic ASTs have no source
        // and start empty.
        let capacity = comments.source_text().map_or(0, |source| source.len().div_ceil(8));
        Self {
            elements: Vec::with_capacity(capacity),
            next_group_id: 0,
            options,
            comments,
            sorting_blocks: FxHashSet::default(),
        }
    }

    pub(crate) fn sorting_requires_in(mut self, blocks: FxHashSet<Span>) -> Self {
        self.sorting_blocks = blocks;
        self
    }

    pub(crate) fn sorts_requires_in(&self, block: &Block) -> bool {
        self.sorting_blocks.contains(&block.span)
    }

    /// Where the next element goes, which an emitter notes to find what it
    /// wrote.
    pub(crate) const fn element_count(&self) -> usize {
        self.elements.len()
    }

    /// Prints the statements written to `slices`, element ranges in stream
    /// order, in `order`, which indexes `slices`. What lies between two of
    /// them stays in place.
    pub(crate) fn reorder(&mut self, slices: &[Range<usize>], order: &[usize]) {
        let (Some(first), Some(last)) = (slices.first(), slices.last()) else {
            return;
        };
        let tail = self.elements.split_off(last.end);
        let mut written = self.elements.split_off(first.start).into_iter();
        let mut gaps = Vec::with_capacity(slices.len());
        let mut pieces = Vec::with_capacity(slices.len());
        let mut position = first.start;
        for slice in slices {
            gaps.push(
                written.by_ref().take(slice.start.saturating_sub(position)).collect::<Vec<_>>(),
            );
            pieces.push(written.by_ref().take(slice.len()).collect::<Vec<_>>());
            position = slice.end;
        }
        for (gap, &i) in gaps.into_iter().zip(order) {
            self.elements.extend(gap);
            if let Some(piece) = pieces.get_mut(i) {
                self.elements.append(piece);
            }
        }
        self.elements.extend(tail);
    }

    pub(crate) const fn options(&self) -> &FormatOptions {
        &self.options
    }

    pub(crate) const fn comments(&self) -> &Comments {
        &self.comments
    }

    pub(crate) const fn comments_mut(&mut self) -> &mut Comments {
        &mut self.comments
    }

    pub(crate) const fn group_id(&mut self) -> GroupId {
        let id = GroupId(self.next_group_id);
        self.next_group_id = self.next_group_id.saturating_add(1);
        id
    }

    #[inline]
    fn push(&mut self, element: FormatElement) {
        self.elements.push(element);
    }

    /// Moves `content` into the stream, where a reusable [`Format`] value
    /// would have to copy it.
    pub(crate) fn write_text(&mut self, content: impl Into<CompactString>) {
        let text = content.into();
        let width = TextWidth::of(&text);
        self.elements.push(FormatElement::Text(text, width));
    }

    /// Text ending in a line break leaves the next line to the printer's
    /// indent. Any other starts at its own column.
    pub(crate) fn write_verbatim(&mut self, content: impl Into<CompactString>) {
        let text = content.into();
        let width = TextWidth::of(&text);
        self.elements.push(FormatElement::Verbatim(text, width));
    }

    /// Writes a space and `text` as a line suffix, printed just before the
    /// next line break, which is how a trailing comment stays at its line's
    /// end.
    ///
    /// A suffix body is only ever this, so the printer can flush one without
    /// laying it out.
    pub(crate) fn write_line_suffix(&mut self, text: CompactString) {
        self.push(FormatElement::Tag(Tag::StartLineSuffix));
        self.push(FormatElement::Space);
        self.write_text(text);
        self.push(FormatElement::Tag(Tag::EndLineSuffix));
    }

    #[cfg(test)]
    pub(crate) fn elements(&self) -> &[FormatElement] {
        &self.elements
    }

    pub(crate) const fn checkpoint(&self) -> Checkpoint {
        Checkpoint { element_count: self.elements.len(), comments: self.comments.checkpoint() }
    }

    pub(crate) fn restore(&mut self, checkpoint: Checkpoint) {
        self.elements.truncate(checkpoint.element_count);
        self.comments.restore(checkpoint.comments);
    }

    pub(crate) fn print(self, base_indent: &str) -> String {
        printer::print(&self.elements, self.next_group_id, &self.options, base_indent)
    }
}

#[derive(Debug)]
pub(crate) struct FormatWith<F>(F);

impl<F: Fn(&mut Formatter)> Format for FormatWith<F> {
    fn fmt(&self, f: &mut Formatter) {
        (self.0)(f);
    }
}

pub(crate) const fn format_with<F>(build: F) -> FormatWith<F>
where
    F: Fn(&mut Formatter),
{
    FormatWith(build)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StaticToken(&'static str);

impl Format for StaticToken {
    fn fmt(&self, f: &mut Formatter) {
        f.push(FormatElement::Token(self.0));
    }
}

pub(crate) const fn token(content: &'static str) -> StaticToken {
    debug_assert!(content.is_ascii(), "a token is measured by its length");
    StaticToken(content)
}

macro_rules! atom {
    ($function_name:ident, $element:expr) => {
        pub(crate) fn $function_name() -> impl Format {
            format_with(|f| f.push($element))
        }
    };
}
atom!(space, FormatElement::Space);
atom!(soft_line, FormatElement::Line(LineMode::Soft));
atom!(soft_line_or_space, FormatElement::Line(LineMode::SoftOrSpace));
atom!(hard_line, FormatElement::Line(LineMode::Hard));
atom!(empty_line, FormatElement::Line(LineMode::Empty));
atom!(expand_parent, FormatElement::ExpandParent);
atom!(line_suffix_boundary, FormatElement::LineSuffixBoundary);

pub(crate) fn indent<T>(content: T) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartIndent));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndIndent));
    })
}

pub(crate) fn group<T>(content: T) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        let id = f.group_id();
        f.push(FormatElement::Tag(Tag::StartGroup(id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndGroup));
    })
}

/// `count` groups, each inside the one before, all opened before `content`.
/// It ends them innermost first through the [`OpenGroups`] it gets, and any it
/// leaves open end after it.
///
/// Laid out this way, the groups wrap what they share at the front, as a
/// method chain does its receiver, without nesting a call per group.
pub(crate) fn nested_groups<Content>(count: usize, content: Content) -> impl Format
where
    Content: Fn(&mut Formatter, &mut OpenGroups),
{
    format_with(move |f| {
        for _ in 0..count {
            let id = f.group_id();
            f.push(FormatElement::Tag(Tag::StartGroup(id)));
        }
        let mut groups = OpenGroups { open: count };
        content(f, &mut groups);
        for _ in 0..groups.open {
            f.push(FormatElement::Tag(Tag::EndGroup));
        }
    })
}

/// The groups of a [`nested_groups`] still open.
#[derive(Debug)]
pub(crate) struct OpenGroups {
    open: usize,
}

impl OpenGroups {
    /// Ends the innermost group still open. Content between ends has to
    /// close every group of its own first.
    pub(crate) fn end_innermost(&mut self, f: &mut Formatter) {
        if let Some(remaining) = self.open.checked_sub(1) {
            self.open = remaining;
            f.push(FormatElement::Tag(Tag::EndGroup));
        }
    }
}

/// Group with a caller-allocated id, so other elements can reference its
/// break decision via `if_group_breaks`.
pub(crate) fn group_with_id<T>(id: GroupId, content: T) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartGroup(id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndGroup));
    })
}

/// See [`Tag::StartNextLineGroup`].
pub(crate) fn next_line_group<T>(content: T) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        let id = f.group_id();
        f.push(FormatElement::Tag(Tag::StartNextLineGroup(id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndNextLineGroup));
    })
}

/// See [`Tag::StartHuggedGroup`].
pub(crate) fn hugged_group_with_id<T>(id: GroupId, content: T) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartHuggedGroup(id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndGroup));
    })
}

/// The group must start before this element, as for `if_group_breaks`.
pub(crate) fn indent_if_group_breaks<T>(group_id: GroupId, content: T) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartIndentIfGroupBreaks(group_id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndIndentIfGroupBreaks(group_id)));
    })
}

pub(crate) fn flat<T>(content: T) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartFlat));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndFlat));
    })
}

/// The group must start before this element (enclosing or earlier sibling).
pub(crate) fn if_group_breaks<T>(group_id: GroupId, content: T) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartIfGroupBreaks(group_id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndIfGroupBreaks));
    })
}

pub(crate) fn separated<T, Separator>(items: &[T], separator: Separator) -> impl Format
where
    T: Format,
    Separator: Format,
{
    format_with(move |f| {
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                separator.fmt(f);
            }
            item.fmt(f);
        }
    })
}

/// Pack entries onto lines greedily.
///
/// Each entry comes with the line that parts it from the one before, which
/// the first entry has no use for. Entry boundaries are explicit so the
/// printer can measure each entry as a unit.
pub(crate) fn fill<T>(entries: &[(LineMode, T)]) -> impl Format
where
    T: Format,
{
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartFill));
        for (i, (separator, entry)) in entries.iter().enumerate() {
            if i > 0 {
                f.push(FormatElement::Line(*separator));
            }
            f.push(FormatElement::Tag(Tag::StartEntry));
            entry.fmt(f);
            f.push(FormatElement::Tag(Tag::EndEntry));
        }
        f.push(FormatElement::Tag(Tag::EndFill));
    })
}
