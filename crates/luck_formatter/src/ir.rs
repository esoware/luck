//! Formatter IR: a Wadler/Prettier document algebra.
//!
//! The document is a flat element stream; structure is expressed by paired
//! `Tag` start/end markers rather than nested ownership, which keeps
//! traversal (printing, measuring, expand propagation) allocation-free.
//!
//! Emission goes through the [`Format`] trait plus the combinators in this
//! module - emitters compose values that know how to write themselves,
//! instead of pushing raw elements.

use compact_str::CompactString;

/// Identifies a group so conditional content and indents can reference the
/// break decision of a group other than their innermost enclosing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(pub u32);

/// How a line element behaves in flat vs expanded mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMode {
    /// Nothing when flat, newline when expanded.
    Soft,
    /// Space when flat, newline when expanded.
    SoftOrSpace,
    /// Always a newline; forces every enclosing group to expand.
    Hard,
    /// Always a blank line (two newlines); forces expansion like `Hard`.
    Empty,
}

/// Whether a group prints on one line or breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintMode {
    Flat,
    Expanded,
}

/// Paired structural markers. Every `Start*` has a matching `End*`;
/// the builder combinators guarantee pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    /// Content that prints flat if it fits the remaining line width.
    StartGroup(GroupId),
    EndGroup,
    StartIndent,
    EndIndent,
    /// Indent by an explicit number of spaces (independent of indent style).
    StartAlign(u8),
    EndAlign,
    /// Content emitted only when the referenced group took `mode`.
    StartConditional {
        mode: PrintMode,
        group_id: GroupId,
    },
    EndConditional,
    /// Indent applied only when the referenced group expanded.
    StartIndentIfGroupBreaks(GroupId),
    EndIndentIfGroupBreaks,
    /// Fill: pack as many entries per line as fit. Entries are wrapped in
    /// `StartEntry`/`EndEntry`; separators sit between entries.
    StartFill,
    EndFill,
    StartEntry,
    EndEntry,
    /// Content deferred to just before the next line break (trailing comments).
    StartLineSuffix,
    EndLineSuffix,
}

/// One element of the document stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatElement {
    /// Static token text - keywords, operators, punctuation.
    Token(&'static str),
    /// Dynamic text - identifiers, literals, comment bodies. May contain
    /// newlines (long strings); the printer measures from the last line.
    Text(CompactString),
    Space,
    Line(LineMode),
    /// Force every enclosing group to expand without printing anything.
    ExpandParent,
    /// Flush pending line suffixes here even without a line break.
    LineSuffixBoundary,
    /// Layout variants ordered most-flat first; the printer takes the first
    /// variant that fits, or the last if none fit. Acts as an expansion
    /// boundary: breaks inside variants don't force outer groups.
    BestFitting(Vec<Vec<FormatElement>>),
    Tag(Tag),
}

/// Anything that can write itself into the IR stream.
pub trait Format {
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

impl Format for () {
    fn fmt(&self, _f: &mut Formatter) {}
}

macro_rules! tuple_format {
    ($($name:ident)+) => {
        impl<$($name: Format),+> Format for ($($name,)+) {
            fn fmt(&self, f: &mut Formatter) {
                #[allow(non_snake_case)]
                let ($($name,)+) = self;
                $($name.fmt(f);)+
            }
        }
    };
}
tuple_format!(A);
tuple_format!(A B);
tuple_format!(A B C);
tuple_format!(A B C D);
tuple_format!(A B C D E);
tuple_format!(A B C D E F);
tuple_format!(A B C D E F G);
tuple_format!(A B C D E F G H);
tuple_format!(A B C D E F G H I);
tuple_format!(A B C D E F G H I J);
tuple_format!(A B C D E F G H I J K);
tuple_format!(A B C D E F G H I J K L);

/// Write a sequence of formatable values: `write!(f, [a, b, c])`.
#[macro_export]
macro_rules! write {
    ($f:expr, [$($arg:expr),* $(,)?]) => {{
        $($crate::ir::Format::fmt(&$arg, $f);)*
    }};
}

/// Rollback point for speculative formatting. The element stream and the
/// comment cursor are rewound - group ids stay unique across restores so
/// references made before the rollback remain valid.
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint {
    element_len: usize,
    comments: usize,
}

/// The document builder threaded through every emitter. Carries the
/// language context (options, comments) alongside the element stream -
/// luck formats one language, so no generic context indirection.
pub struct Formatter {
    elements: Vec<FormatElement>,
    next_group_id: u32,
    pub options: crate::FormatOptions,
    pub comments: crate::comments::Comments,
    /// When set, only statements overlapping this byte range are formatted;
    /// the rest emit verbatim (editor "format selection"). Requires source.
    pub format_range: Option<std::ops::Range<u32>>,
}

impl Formatter {
    pub fn new() -> Self {
        Self::with_context(
            crate::FormatOptions::default(),
            crate::comments::Comments::none(),
        )
    }

    pub fn with_context(
        options: crate::FormatOptions,
        comments: crate::comments::Comments,
    ) -> Self {
        // Roughly one element per 8 source bytes; a mild undershoot that
        // still removes most realloc copies. Synthetic ASTs have no source
        // and start empty.
        let elements = match comments.source_text() {
            Some(source) => Vec::with_capacity(source.len() / 8),
            None => Vec::new(),
        };
        Self {
            elements,
            next_group_id: 0,
            options,
            comments,
            format_range: None,
        }
    }

    pub fn group_id(&mut self) -> GroupId {
        let id = GroupId(self.next_group_id);
        self.next_group_id += 1;
        id
    }

    /// Total number of group ids handed out; sizes the printer's mode table.
    pub fn group_count(&self) -> u32 {
        self.next_group_id
    }

    #[inline]
    pub fn push(&mut self, element: FormatElement) {
        self.elements.push(element);
    }

    pub fn into_elements(self) -> Vec<FormatElement> {
        self.elements
    }

    pub fn elements(&self) -> &[FormatElement] {
        &self.elements
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            element_len: self.elements.len(),
            comments: self.comments.checkpoint(),
        }
    }

    pub fn restore(&mut self, checkpoint: Checkpoint) {
        self.elements.truncate(checkpoint.element_len);
        self.comments.restore(checkpoint.comments);
    }

    /// Detach the elements written since `checkpoint` (used by
    /// `best_fitting` to capture variants).
    pub fn take_since(&mut self, checkpoint: Checkpoint) -> Vec<FormatElement> {
        self.elements.split_off(checkpoint.element_len)
    }
}

impl Default for Formatter {
    fn default() -> Self {
        Self::new()
    }
}

/// Adapter turning a closure into a `Format` value.
pub struct FormatWith<F>(F);

impl<F: Fn(&mut Formatter)> Format for FormatWith<F> {
    fn fmt(&self, f: &mut Formatter) {
        (self.0)(f);
    }
}

pub fn format_with<F: Fn(&mut Formatter)>(build: F) -> FormatWith<F> {
    FormatWith(build)
}

pub struct StaticToken(&'static str);

impl Format for StaticToken {
    fn fmt(&self, f: &mut Formatter) {
        f.push(FormatElement::Token(self.0));
    }
}

pub fn token(content: &'static str) -> StaticToken {
    StaticToken(content)
}

pub struct DynamicText(CompactString);

impl Format for DynamicText {
    fn fmt(&self, f: &mut Formatter) {
        f.push(FormatElement::Text(self.0.clone()));
    }
}

pub fn text(content: impl Into<CompactString>) -> DynamicText {
    DynamicText(content.into())
}

macro_rules! atom {
    ($fn_name:ident, $element:expr) => {
        pub fn $fn_name() -> impl Format {
            format_with(|f| f.push($element))
        }
    };
}
atom!(space, FormatElement::Space);
atom!(soft_line, FormatElement::Line(LineMode::Soft));
atom!(
    soft_line_or_space,
    FormatElement::Line(LineMode::SoftOrSpace)
);
atom!(hard_line, FormatElement::Line(LineMode::Hard));
atom!(empty_line, FormatElement::Line(LineMode::Empty));
atom!(expand_parent, FormatElement::ExpandParent);
atom!(line_suffix_boundary, FormatElement::LineSuffixBoundary);

macro_rules! tagged {
    ($fn_name:ident, $start:expr, $end:expr) => {
        pub fn $fn_name<T: Format>(content: T) -> impl Format {
            format_with(move |f| {
                f.push(FormatElement::Tag($start));
                content.fmt(f);
                f.push(FormatElement::Tag($end));
            })
        }
    };
}
tagged!(indent, Tag::StartIndent, Tag::EndIndent);
tagged!(line_suffix, Tag::StartLineSuffix, Tag::EndLineSuffix);

pub fn align<T: Format>(width: u8, content: T) -> impl Format {
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartAlign(width)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndAlign));
    })
}

/// Group with a fresh id allocated at write time.
pub fn group<T: Format>(content: T) -> impl Format {
    format_with(move |f| {
        let id = f.group_id();
        f.push(FormatElement::Tag(Tag::StartGroup(id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndGroup));
    })
}

/// Group with a caller-allocated id, so other elements can reference its
/// break decision via `if_group_breaks`/`indent_if_group_breaks`.
pub fn group_with_id<T: Format>(id: GroupId, content: T) -> impl Format {
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartGroup(id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndGroup));
    })
}

/// Emitted only when the referenced group breaks. The group must start
/// before this element (enclosing or earlier sibling).
pub fn if_group_breaks<T: Format>(group_id: GroupId, content: T) -> impl Format {
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartConditional {
            mode: PrintMode::Expanded,
            group_id,
        }));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndConditional));
    })
}

/// Emitted only when the referenced group prints flat.
pub fn if_group_fits<T: Format>(group_id: GroupId, content: T) -> impl Format {
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartConditional {
            mode: PrintMode::Flat,
            group_id,
        }));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndConditional));
    })
}

pub fn indent_if_group_breaks<T: Format>(group_id: GroupId, content: T) -> impl Format {
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartIndentIfGroupBreaks(group_id)));
        content.fmt(f);
        f.push(FormatElement::Tag(Tag::EndIndentIfGroupBreaks));
    })
}

/// Fill: pack entries onto lines greedily, separated by `separator`
/// (typically `SoftOrSpace`). Entry boundaries are explicit so the printer
/// can measure each entry as a unit.
pub fn fill<'a>(separator: LineMode, entries: &'a [&'a dyn Format]) -> impl Format + 'a {
    format_with(move |f| {
        f.push(FormatElement::Tag(Tag::StartFill));
        for (index, entry) in entries.iter().enumerate() {
            if index > 0 {
                f.push(FormatElement::Line(separator));
            }
            f.push(FormatElement::Tag(Tag::StartEntry));
            entry.fmt(f);
            f.push(FormatElement::Tag(Tag::EndEntry));
        }
        f.push(FormatElement::Tag(Tag::EndFill));
    })
}

/// Layout variants, most-flat first. The printer takes the first that fits.
pub struct BestFitting<'a> {
    variants: &'a [&'a dyn Format],
}

impl Format for BestFitting<'_> {
    fn fmt(&self, f: &mut Formatter) {
        debug_assert!(
            self.variants.len() >= 2,
            "best_fitting needs at least two variants"
        );
        let mut captured = Vec::with_capacity(self.variants.len());
        for variant in self.variants {
            let checkpoint = f.checkpoint();
            variant.fmt(f);
            captured.push(f.take_since(checkpoint));
        }
        f.push(FormatElement::BestFitting(captured));
    }
}

pub fn best_fitting<'a>(variants: &'a [&'a dyn Format]) -> BestFitting<'a> {
    BestFitting { variants }
}
