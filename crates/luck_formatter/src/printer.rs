//! One pass over the stream, guided by the [`Layout`] the formatter records.
//!
//! As the formatter writes, every group that contains a forced break (a hard
//! or empty line, `ExpandParent`, or multi-line text) is marked expanded as
//! the break goes in, since such a group can never print flat and isn't worth
//! measuring.
//!
//! The print loop then walks the stream once with a mode stack. Entering an
//! undecided group measures whether its content fits the remaining width and
//! commits to `Flat` or `Expanded`, and each decision is recorded by `GroupId`
//! so conditional content elsewhere can look it up.
//!
//! A group holding hugged groups has two more layouts, tried when it doesn't
//! fit flat. It stays flat while hugged groups expand in its place: first
//! only those a forced break expands anyway, then all of them. That is how
//! `f(a, function()` keeps its head on one line.
//!
//! A next-line group that doesn't fit flat where it starts is measured again
//! from the start of the next line, one indent deeper. If it fits there, the
//! printer breaks the line before it and prints it flat.

use core::mem;
use core::ops::Range;

use luck_allocator::{Allocator, List};
use luck_token::{CodeBuffer, index_to_offset, offset_to_index};
use unicode_width::UnicodeWidthStr as _;

use crate::ir::{FormatElement, GroupId, LineMode, TextWidth};
use crate::{FormatOptions, IndentStyle, LineEndings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrintMode {
    Flat,
    Expanded,
}

/// A hugged group, as the group right around it sees it. Both offsets count
/// elements from that group's first.
#[derive(Debug, Clone, Copy)]
struct Hug {
    child: GroupId,
    /// The first line directly inside the child, where its opening line ends.
    first_line: usize,
    /// The last such line, where its closing line starts.
    last_line: usize,
}

#[derive(Debug, Clone, Copy)]
struct Group {
    /// `Expanded` for a group a forced break expands, and filled in as the
    /// printer enters the group otherwise.
    mode: Option<PrintMode>,
    /// The hugged groups this one holds, in order, in `Layout::hugs`.
    hugs_start: u32,
    hugs_end: u32,
}

#[derive(Debug)]
struct OpenGroup {
    id: GroupId,
    /// Index of the group's first element.
    content_start: usize,
    is_hugged: bool,
    /// Where its hugs start in `Layout::pending`.
    pending_hugs: usize,
    /// For a hugged group, the first and last lines directly inside it, as
    /// the group around it counts them.
    hug_lines: Option<(usize, usize)>,
    /// The element a forced break marked the group expanded at.
    marked_at: Option<usize>,
}

/// What the printer has to know about each group before it reaches it, kept
/// up to date as the stream is written.
#[derive(Debug)]
pub(crate) struct Layout<'ast> {
    groups: List<'ast, Group>,
    hugs: Vec<Hug>,
    open: Vec<OpenGroup>,
    /// The finished hugs of each open group, innermost last. A group's are
    /// contiguous, since each group inside it moves its own out as it closes,
    /// before its parent's hug of it goes in.
    pending: Vec<Hug>,
}

/// Where the layout stood when the stream was at a checkpoint.
///
/// Every group opened since has closed by the time it's restored, and only an
/// open group can be marked, so only the marks on the groups still open, the
/// hugs, and the lines the innermost one has seen can have moved.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LayoutCheckpoint {
    element_count: usize,
    hugs: usize,
    pending: usize,
    hug_lines: Option<(usize, usize)>,
}

impl<'ast> Layout<'ast> {
    pub(crate) fn with_capacity_in(group_count: usize, allocator: &'ast Allocator) -> Self {
        Self {
            groups: List::with_capacity_in(group_count, allocator),
            hugs: Vec::new(),
            open: Vec::new(),
            pending: Vec::new(),
        }
    }

    pub(crate) fn new_group(&mut self, allocator: &'ast Allocator) -> GroupId {
        let id = GroupId::new(self.groups.len());
        self.groups.push(Group { mode: None, hugs_start: 0, hugs_end: 0 }, allocator);
        id
    }

    /// `content_start` is the index of the group's first element.
    pub(crate) fn open(&mut self, id: GroupId, content_start: usize, is_hugged: bool) {
        let pending_hugs = self.pending.len();
        self.open.push(OpenGroup {
            id,
            content_start,
            is_hugged,
            pending_hugs,
            hug_lines: None,
            marked_at: None,
        });
    }

    pub(crate) fn close(&mut self) {
        let Some(closed) = self.open.pop() else {
            return;
        };
        if closed.pending_hugs < self.pending.len()
            && let Some(group) = self.groups.get_mut(closed.id.index())
        {
            group.hugs_start = index_to_offset(self.hugs.len());
            self.hugs.extend(self.pending.drain(closed.pending_hugs..));
            group.hugs_end = index_to_offset(self.hugs.len());
        }
        if let Some((first_line, last_line)) = closed.hug_lines {
            self.pending.push(Hug { child: closed.id, first_line, last_line });
        }
    }

    /// A line at element `index`. One directly inside a hugged group marks
    /// where the group's lines start and end.
    pub(crate) fn line(&mut self, index: usize, mode: LineMode) {
        if let [.., parent, child] = self.open.as_mut_slice()
            && child.is_hugged
        {
            let line = index.saturating_sub(parent.content_start);
            let first_line = child.hug_lines.map_or(line, |(first_line, _)| first_line);
            child.hug_lines = Some((first_line, line));
        }
        if matches!(mode, LineMode::Hard | LineMode::Empty) {
            self.force_break(index);
        }
    }

    /// A forced break marks every open group, from the innermost out, and
    /// stops at one already marked.
    ///
    /// The exception is the group right around a hugged group the break is
    /// in, which can stay flat while the hugged group breaks. The break that
    /// marked a group settled every group around it, and each of those is
    /// still open around the same inner group, so the same hug still exempts
    /// it.
    ///
    /// `index` is the element the break is.
    pub(crate) fn force_break(&mut self, index: usize) {
        let mut is_inner_hugged = false;
        for open in self.open.iter_mut().rev() {
            if !is_inner_hugged && let Some(group) = self.groups.get_mut(open.id.index()) {
                if group.mode == Some(PrintMode::Expanded) {
                    return;
                }
                group.mode = Some(PrintMode::Expanded);
                open.marked_at = Some(index);
            }
            is_inner_hugged = open.is_hugged;
        }
    }

    /// `element_count` is how many elements the stream holds now.
    pub(crate) fn checkpoint(&self, element_count: usize) -> LayoutCheckpoint {
        LayoutCheckpoint {
            element_count,
            hugs: self.hugs.len(),
            pending: self.pending.len(),
            hug_lines: self.open.last().and_then(|open| open.hug_lines),
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: LayoutCheckpoint) {
        for open in &mut self.open {
            if open.marked_at.is_some_and(|index| index >= checkpoint.element_count)
                && let Some(group) = self.groups.get_mut(open.id.index())
            {
                group.mode = None;
                open.marked_at = None;
            }
        }
        self.hugs.truncate(checkpoint.hugs);
        self.pending.truncate(checkpoint.pending);
        if let Some(open) = self.open.last_mut() {
            open.hug_lines = checkpoint.hug_lines;
        }
    }
}

pub(crate) fn print(
    elements: &[FormatElement<'_>],
    layout: Layout<'_>,
    options: &FormatOptions,
    base_indent: &str,
    capacity: usize,
) -> String {
    let columns_per_indent = usize::from(options.indent_width);
    let (indent_byte, bytes_per_indent) = match options.indent_style {
        IndentStyle::Tabs => (b'\t', 1),
        IndentStyle::Spaces => (b' ', columns_per_indent),
    };
    let base_indent_columns = base_indent
        .bytes()
        .map(|b| if b == b'\t' { columns_per_indent } else { 1 })
        .fold(0usize, usize::saturating_add);
    let mut printer = Printer {
        line_width: usize::from(options.line_width),
        indent_byte,
        bytes_per_indent,
        columns_per_indent,
        base_indent,
        base_indent_columns,
        output: CodeBuffer::with_capacity(capacity),
        is_crlf: options.line_endings == LineEndings::Windows,
        crlf_end: None,
        line_start: 0,
        display_column: 0,
        indent_level: 0,
        modes: Vec::new(),
        groups: layout.groups,
        hugs: layout.hugs,
        next_line_moves: Vec::new(),
        suffixes: Vec::new(),
        is_pending_space: false,
        is_pending_indent: !base_indent.is_empty(),
    };
    printer.print_elements(elements);
    printer.flush_suffixes();
    printer.output.into_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    Group,
    Entry,
}

struct Printer<'ast, 'layout> {
    line_width: usize,
    /// One indent level is `bytes_per_indent` copies of `indent_byte` and
    /// measures `columns_per_indent` columns, so a tab is one byte and as wide
    /// as `indent_width` says.
    indent_byte: u8,
    bytes_per_indent: usize,
    columns_per_indent: usize,
    /// Written before the indent of every line.
    base_indent: &'ast str,
    /// A tab in `base_indent` measures `columns_per_indent` too.
    base_indent_columns: usize,
    output: CodeBuffer,
    /// Each LF that isn't half of a CRLF or an LFCR prints as CRLF. Verbatim
    /// text and long strings can hold either already, and another CR would
    /// be another line break.
    is_crlf: bool,
    /// Where the latest CRLF made of an LF ends. A CR written right after it
    /// makes it an LF again, as half of an LFCR.
    crlf_end: Option<usize>,
    line_start: usize,
    display_column: usize,
    indent_level: usize,
    /// Effective print-mode stack; empty means `Expanded` (top level).
    modes: Vec<PrintMode>,
    groups: List<'layout, Group>,
    hugs: Vec<Hug>,
    /// Per open next-line group, whether it moved to the next line, so its
    /// end takes back the indent it added.
    next_line_moves: Vec<bool>,
    suffixes: Vec<(&'ast str, TextWidth)>,
    is_pending_space: bool,
    is_pending_indent: bool,
}

impl<'ast> Printer<'ast, '_> {
    fn current_mode(&self) -> PrintMode {
        *self.modes.last().unwrap_or(&PrintMode::Expanded)
    }

    fn group_mode(&self, id: GroupId) -> Option<PrintMode> {
        self.groups.get(id.index())?.mode
    }

    fn set_group_mode(&mut self, id: GroupId, mode: PrintMode) {
        if let Some(group) = self.groups.get_mut(id.index()) {
            group.mode = Some(mode);
        }
    }

    fn print_elements(&mut self, elements: &'ast [FormatElement<'ast>]) {
        let mut remaining = elements;
        while let Some((element, rest)) = remaining.split_first() {
            remaining = self.print_element(element, rest);
        }
    }

    /// Print `element`, returning the elements left after it and after any
    /// region it consumed from `rest`, e.g. a skipped conditional body.
    #[expect(
        clippy::inline_always,
        reason = "measured: a call per element costs the print loop 7%"
    )]
    #[inline(always)]
    fn print_element(
        &mut self,
        element: &FormatElement<'ast>,
        rest: &'ast [FormatElement<'ast>],
    ) -> &'ast [FormatElement<'ast>] {
        match *element {
            FormatElement::Text(content, width) => {
                self.emit_text(content, width.columns());
                self.is_pending_space |= width.has_space_after();
            }
            FormatElement::Verbatim(content, width) => self.emit_verbatim(content, width.columns()),
            FormatElement::LineSuffix(content, width) => self.suffixes.push((content, width)),
            FormatElement::Space => self.is_pending_space = true,
            FormatElement::Line(mode) => self.print_line(mode),
            FormatElement::LineSuffixBoundary => {
                if !self.suffixes.is_empty() {
                    self.emit_newline(false);
                }
            }
            FormatElement::StartGroup(id) | FormatElement::StartHuggedGroup(id) => {
                // A mode already set was forced by a break in it, or by
                // the group around a hugged one.
                let mode = self.group_mode(id).unwrap_or_else(|| self.decide_mode(id, rest));
                self.set_group_mode(id, mode);
                self.modes.push(mode);
            }
            FormatElement::StartNextLineGroup(id) => {
                let next_line_indent =
                    self.indent_columns().saturating_add(self.columns_per_indent);
                let (mode, is_moved) = match self.group_mode(id) {
                    Some(forced) => (forced, false),
                    None if self.current_mode() == PrintMode::Flat
                        || self.fits(rest, Region::Group, 0) =>
                    {
                        (PrintMode::Flat, false)
                    }
                    None if self.fits_from(rest, Region::Group, next_line_indent, 1) => {
                        (PrintMode::Flat, true)
                    }
                    None => (PrintMode::Expanded, false),
                };
                self.set_group_mode(id, mode);
                if is_moved {
                    self.emit_newline(false);
                    self.indent_level = self.indent_level.saturating_add(1);
                    // Written now, so an indent inside the flat content
                    // can't deepen this line.
                    self.flush_indent();
                }
                self.next_line_moves.push(is_moved);
                self.modes.push(mode);
            }
            FormatElement::EndNextLineGroup => {
                self.modes.pop();
                if self.next_line_moves.pop() == Some(true) {
                    self.indent_level = self.indent_level.saturating_sub(1);
                }
            }
            FormatElement::StartFlat => self.modes.push(PrintMode::Flat),
            FormatElement::EndGroup | FormatElement::EndFlat => {
                self.modes.pop();
            }
            FormatElement::StartIndent => self.indent_level = self.indent_level.saturating_add(1),
            FormatElement::EndIndent => self.indent_level = self.indent_level.saturating_sub(1),
            FormatElement::StartIndentIfGroupBreaks(id) => {
                if self.group_mode(id) == Some(PrintMode::Expanded) {
                    self.indent_level = self.indent_level.saturating_add(1);
                }
            }
            FormatElement::EndIndentIfGroupBreaks(id) => {
                if self.group_mode(id) == Some(PrintMode::Expanded) {
                    self.indent_level = self.indent_level.saturating_sub(1);
                }
            }
            FormatElement::StartIfGroupBreaks(id) => {
                let decided_mode = self.group_mode(id);
                debug_assert!(
                    decided_mode.is_some(),
                    "conditional references group {id:?} before its decision"
                );
                if decided_mode != Some(PrintMode::Expanded) {
                    return skip_conditional(rest);
                }
            }
            FormatElement::StartFill => return self.print_fill(rest),
            // `ExpandParent` only steers the `Layout`, and a taken
            // conditional's end is a no-op. `print_fill` consumes the other
            // elements listed here, so only malformed IR reaches this arm
            // with one of them.
            FormatElement::ExpandParent
            | FormatElement::EndIfGroupBreaks
            | FormatElement::EndFill
            | FormatElement::StartEntry
            | FormatElement::EndEntry => {}
        }
        rest
    }

    /// `rest` starts at the group's first element.
    fn decide_mode(&mut self, id: GroupId, rest: &[FormatElement<'_>]) -> PrintMode {
        // An enclosing flat group already measured this content;
        // re-measuring can't change the answer.
        if self.current_mode() == PrintMode::Flat || self.fits(rest, Region::Group, 0) {
            return PrintMode::Flat;
        }
        let hugs = self.groups.get(id.index()).map_or(0..0, |group| {
            offset_to_index(group.hugs_start)..offset_to_index(group.hugs_end)
        });
        if hugs.is_empty() {
            return PrintMode::Expanded;
        }
        // A hugged group left undecided here prints flat, like anything else
        // in a flat group. With none forced, that's the flat layout that just
        // failed.
        let is_any_hug_forced = self
            .hugs
            .get(hugs.clone())
            .unwrap_or_default()
            .iter()
            .any(|hug| self.group_mode(hug.child) == Some(PrintMode::Expanded));
        if is_any_hug_forced && self.fits_hugging(rest, hugs.clone(), false) {
            return PrintMode::Flat;
        }
        if self.fits_hugging(rest, hugs.clone(), true) {
            for i in hugs {
                if let Some(hug) = self.hugs.get(i).copied() {
                    self.set_group_mode(hug.child, PrintMode::Expanded);
                }
            }
            return PrintMode::Flat;
        }
        PrintMode::Expanded
    }

    fn print_line(&mut self, mode: LineMode) {
        match (self.current_mode(), mode) {
            (PrintMode::Flat, LineMode::Soft) => {}
            (PrintMode::Flat, LineMode::SoftOrSpace) => self.is_pending_space = true,
            // Propagation expands every group around a forced break, so one
            // can't sit in a flat region, but break anyway to be safe.
            (_, LineMode::Hard) | (PrintMode::Expanded, LineMode::Soft | LineMode::SoftOrSpace) => {
                self.emit_newline(false);
            }
            (_, LineMode::Empty) => self.emit_newline(true),
        }
    }

    /// Greedy fill: each entry goes on the current line if it fits,
    /// otherwise the preceding separator becomes a newline. Entries that
    /// don't fit even on a fresh line print expanded.
    fn print_fill(&mut self, elements: &'ast [FormatElement<'ast>]) -> &'ast [FormatElement<'ast>] {
        let mut remaining = elements;
        while let Some((element, rest)) = remaining.split_first() {
            match element {
                FormatElement::EndFill => return rest,
                FormatElement::Line(separator) => {
                    debug_assert!(
                        matches!(rest.first(), Some(FormatElement::StartEntry)),
                        "a fill separator must be followed by an entry"
                    );
                    remaining = rest;
                    let separator_width = match separator {
                        LineMode::SoftOrSpace => 1,
                        LineMode::Soft => 0,
                        LineMode::Hard | LineMode::Empty => {
                            self.emit_newline(matches!(separator, LineMode::Empty));
                            continue;
                        }
                    };
                    let entry_content = rest.get(1..).unwrap_or_default();
                    if self.fits(entry_content, Region::Entry, separator_width) {
                        if matches!(separator, LineMode::SoftOrSpace) {
                            self.is_pending_space = true;
                        }
                    } else {
                        self.emit_newline(false);
                    }
                }
                FormatElement::StartEntry => {
                    let mode = if self.fits(rest, Region::Entry, 0) {
                        PrintMode::Flat
                    } else {
                        PrintMode::Expanded
                    };
                    self.modes.push(mode);
                    // Entry content goes through `print_element`, not this
                    // loop's arms, because a group inside an entry has its own
                    // `Line` elements and the `Line` arm above must only see
                    // separators. A nested fill consumes its own entries, so
                    // the first `EndEntry` here is this entry's.
                    let mut entry_remaining = rest;
                    while let Some((content, after_content)) = entry_remaining.split_first()
                        && !matches!(content, FormatElement::EndEntry)
                    {
                        entry_remaining = self.print_element(content, after_content);
                    }
                    self.modes.pop();
                    remaining = entry_remaining.get(1..).unwrap_or_default();
                }
                FormatElement::Text(..)
                | FormatElement::Verbatim(..)
                | FormatElement::LineSuffix(..)
                | FormatElement::Space
                | FormatElement::ExpandParent
                | FormatElement::LineSuffixBoundary
                | FormatElement::StartGroup(_)
                | FormatElement::StartHuggedGroup(_)
                | FormatElement::EndGroup
                | FormatElement::StartNextLineGroup(_)
                | FormatElement::EndNextLineGroup
                | FormatElement::StartIndent
                | FormatElement::EndIndent
                | FormatElement::StartIndentIfGroupBreaks(_)
                | FormatElement::EndIndentIfGroupBreaks(_)
                | FormatElement::StartIfGroupBreaks(_)
                | FormatElement::EndIfGroupBreaks
                | FormatElement::StartFlat
                | FormatElement::EndFlat
                | FormatElement::StartFill
                | FormatElement::EndEntry => {
                    remaining = self.print_element(element, rest);
                }
            }
        }
        remaining
    }

    fn occupied_width(&self) -> usize {
        // A pending indent hasn't reached `display_column` yet but will occupy the
        // line before any measured content does.
        let pending_indent_columns = if self.is_pending_indent { self.indent_columns() } else { 0 };
        self.display_column
            .saturating_add(pending_indent_columns)
            .saturating_add(usize::from(self.is_pending_space))
    }

    const fn indent_columns(&self) -> usize {
        self.indent_level
            .saturating_mul(self.columns_per_indent)
            .saturating_add(self.base_indent_columns)
    }

    /// Simulate flat printing of `elements` up to the end of `region`,
    /// checking the content fits the remaining width. `separator_width`
    /// charges width up front for a pending separator.
    fn fits(&self, elements: &[FormatElement<'_>], region: Region, separator_width: usize) -> bool {
        let used_width = self.occupied_width().saturating_add(separator_width);
        self.fits_from(elements, region, used_width, 1)
    }

    /// Whether a group fits with its hugged children expanded and the rest
    /// flat.
    ///
    /// Each expanded child ends one line at its first break and starts the
    /// next at its last, so what gets measured is the line up to the first
    /// child, the line from each child to the next, and the line the last
    /// child closes on. Without `should_expand_all`, only the children a
    /// forced break expands count, and the others are measured flat.
    fn fits_hugging(
        &self,
        elements: &[FormatElement<'_>],
        hugs: Range<usize>,
        should_expand_all: bool,
    ) -> bool {
        let mut line_start = 0;
        let mut used_width = self.occupied_width();
        let mut depth = 1;
        for hug in self.hugs.get(hugs).unwrap_or_default() {
            if !should_expand_all && self.group_mode(hug.child) != Some(PrintMode::Expanded) {
                continue;
            }
            let line = elements.get(line_start..hug.first_line).unwrap_or_default();
            if !self.fits_from(line, Region::Group, used_width, depth) {
                return false;
            }
            line_start = hug.last_line.saturating_add(1);
            used_width = self.indent_columns();
            // The next line starts inside the child, two groups deep.
            depth = 2;
        }
        let closing_line = elements.get(line_start..).unwrap_or_default();
        self.fits_from(closing_line, Region::Group, used_width, depth)
    }

    /// `depth` is how many regions deep `elements` starts. Measuring stops
    /// where the outermost of them closes.
    fn fits_from(
        &self,
        elements: &[FormatElement<'_>],
        region: Region,
        mut used_width: usize,
        mut depth: usize,
    ) -> bool {
        let is_group = region == Region::Group;
        let mut remaining = elements;
        while let Some((element, rest)) = remaining.split_first() {
            remaining = rest;
            match *element {
                FormatElement::Text(_, width) | FormatElement::Verbatim(_, width) => {
                    let Some(columns) = width.columns() else {
                        return false;
                    };
                    let space = usize::from(width.has_space_after());
                    used_width = used_width.saturating_add(columns).saturating_add(space);
                    if used_width > self.line_width {
                        return false;
                    }
                }
                FormatElement::Space | FormatElement::Line(LineMode::SoftOrSpace) => {
                    used_width = used_width.saturating_add(1);
                    if used_width > self.line_width {
                        return false;
                    }
                }
                FormatElement::Line(LineMode::Hard | LineMode::Empty)
                | FormatElement::ExpandParent => return false,
                FormatElement::StartIfGroupBreaks(id) => {
                    // A decided group answers with its decision. An undecided
                    // one is being measured now, and prints flat if this
                    // measurement succeeds.
                    if self.group_mode(id) != Some(PrintMode::Expanded) {
                        remaining = skip_conditional(rest);
                    }
                }
                FormatElement::StartGroup(_)
                | FormatElement::StartHuggedGroup(_)
                | FormatElement::StartNextLineGroup(_)
                    if is_group =>
                {
                    depth = depth.saturating_add(1);
                }
                FormatElement::StartEntry if !is_group => depth = depth.saturating_add(1),
                FormatElement::EndGroup | FormatElement::EndNextLineGroup if is_group => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return true;
                    }
                }
                FormatElement::EndEntry if !is_group => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return true;
                    }
                }
                // A line suffix prints past the line's end, so it takes no
                // width here.
                FormatElement::LineSuffix(..)
                | FormatElement::Line(LineMode::Soft)
                | FormatElement::LineSuffixBoundary
                | FormatElement::StartGroup(_)
                | FormatElement::StartHuggedGroup(_)
                | FormatElement::EndGroup
                | FormatElement::StartNextLineGroup(_)
                | FormatElement::EndNextLineGroup
                | FormatElement::StartIndent
                | FormatElement::EndIndent
                | FormatElement::StartIndentIfGroupBreaks(_)
                | FormatElement::EndIndentIfGroupBreaks(_)
                | FormatElement::EndIfGroupBreaks
                | FormatElement::StartFlat
                | FormatElement::EndFlat
                | FormatElement::StartFill
                | FormatElement::EndFill
                | FormatElement::StartEntry
                | FormatElement::EndEntry => {}
            }
        }
        true
    }

    /// `columns` is `None` for text that spans lines.
    fn emit_text(&mut self, content: &str, columns: Option<usize>) {
        if self.is_pending_indent {
            self.flush_indent();
        }
        if self.is_pending_space {
            self.is_pending_space = false;
            self.output.print_ascii_byte(b' ');
            self.display_column = self.display_column.saturating_add(1);
        }
        if self.crlf_end == Some(self.output.len()) {
            self.keep_lf_before_cr(content);
        }
        if columns.is_none() && self.is_crlf {
            self.print_crlf(content);
        } else {
            self.output.print_str(content);
        }
        if let Some(width) = columns {
            self.display_column = self.display_column.saturating_add(width);
        } else {
            // The column restarts from the last line.
            let last_line = content.rsplit_once('\n').map_or(content, |(_, last_line)| last_line);
            self.line_start = self.output.len().saturating_sub(last_line.len());
            self.display_column = last_line.width();
        }
    }

    /// Copy source text, which brings its own indentation and keeps any
    /// trailing whitespace. Text ending in a line break leaves the next line to
    /// the printer's indent.
    fn emit_verbatim(&mut self, content: &str, columns: Option<usize>) {
        self.is_pending_indent = false;
        self.emit_text(content, columns);
        self.line_start = self.output.len();
        self.is_pending_indent = content.ends_with(['\n', '\r']);
    }

    /// The CRLF the output ends in, made of an LF, goes back to an LF when
    /// `content` starts with a CR, as half of an LFCR.
    fn keep_lf_before_cr(&mut self, content: &str) {
        if content.starts_with('\r') {
            self.crlf_end = None;
            self.output.truncate(self.output.len().saturating_sub(2));
            self.output.print_ascii_byte(b'\n');
            self.line_start = self.output.len();
        }
    }

    /// Writes `content`, which spans lines, with each LF that isn't half of
    /// a CRLF or an LFCR as CRLF.
    fn print_crlf(&mut self, content: &str) {
        let bytes = content.as_bytes();
        let mut written = 0;
        let mut is_crlf_last = false;
        for (i, _) in content.match_indices('\n') {
            let previous =
                i.checked_sub(1).map_or_else(|| self.output.as_bytes().last(), |j| bytes.get(j));
            if previous != Some(&b'\r') && bytes.get(i.saturating_add(1)) != Some(&b'\r') {
                self.output.print_str(content.get(written..i).unwrap_or_default());
                self.output.print_ascii_byte(b'\r');
                written = i;
                is_crlf_last = i.saturating_add(1) == bytes.len();
            }
        }
        self.output.print_str(content.get(written..).unwrap_or_default());
        if is_crlf_last {
            self.crlf_end = Some(self.output.len());
        }
    }

    fn emit_newline(&mut self, is_blank_line: bool) {
        self.flush_suffixes();
        self.is_pending_space = false;
        self.trim_trailing_whitespace();
        if self.is_crlf {
            self.print_crlf_line_break(is_blank_line);
        } else {
            // Blank lines never stack, so a break right after one adds nothing.
            while !self.output.as_bytes().ends_with(b"\n\n") {
                self.output.print_ascii_byte(b'\n');
                if !is_blank_line {
                    break;
                }
            }
        }
        self.line_start = self.output.len();
        self.display_column = 0;
        self.is_pending_indent = true;
    }

    /// A line break as CRLF, or as LF after a CR. The output ends in a blank
    /// line when it ends in two LFs, the last of them made CRLF.
    fn print_crlf_line_break(&mut self, is_blank_line: bool) {
        while !(self.crlf_end == Some(self.output.len())
            && self.output.as_bytes().ends_with(b"\n\r\n"))
        {
            if self.output.as_bytes().last() == Some(&b'\r') {
                self.output.print_ascii_byte(b'\n');
            } else {
                self.output.print_str("\r\n");
                self.crlf_end = Some(self.output.len());
            }
            if !is_blank_line {
                break;
            }
        }
    }

    fn flush_suffixes(&mut self) {
        if self.suffixes.is_empty() {
            return;
        }
        let mut suffixes = mem::take(&mut self.suffixes);
        for &(content, width) in &suffixes {
            self.is_pending_space = true;
            self.emit_text(content, width.columns());
        }
        suffixes.clear();
        self.suffixes = suffixes;
    }

    fn trim_trailing_whitespace(&mut self) {
        let bytes = self.output.as_bytes();
        let trailing_whitespace_bytes = bytes
            .get(self.line_start..)
            .unwrap_or_default()
            .iter()
            .rev()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        let trimmed_end = bytes.len().saturating_sub(trailing_whitespace_bytes);
        self.output.truncate(trimmed_end);
    }

    fn flush_indent(&mut self) {
        self.is_pending_indent = false;
        self.output.print_str(self.base_indent);
        let indent_bytes = self.indent_level.saturating_mul(self.bytes_per_indent);
        self.output.print_ascii_repeat(self.indent_byte, indent_bytes);
        self.display_column = self.display_column.saturating_add(self.indent_columns());
    }
}

fn skip_conditional<'elements, 'ast>(
    elements: &'elements [FormatElement<'ast>],
) -> &'elements [FormatElement<'ast>] {
    let mut remaining = elements;
    while let Some((element, rest)) = remaining.split_first() {
        if matches!(element, FormatElement::EndIfGroupBreaks) {
            return rest;
        }
        let is_nested = matches!(element, FormatElement::StartIfGroupBreaks(_));
        remaining = if is_nested { skip_conditional(rest) } else { rest };
    }
    remaining
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;

    use crate::comments::Comments;
    use crate::ir::Formatter;
    use crate::{FormatOptions, IndentStyle};

    fn print_with(
        options: FormatOptions,
        base_indent: &str,
        build: impl Fn(&mut Formatter<'_>),
    ) -> String {
        let allocator = Allocator::new();
        let mut f = Formatter::new(&allocator, options, Comments::none());
        build(&mut f);
        f.print(base_indent)
    }

    fn print_document(build: impl Fn(&mut Formatter<'_>), line_width: u16) -> String {
        let options = FormatOptions {
            line_width,
            indent_style: IndentStyle::Spaces,
            indent_width: 4,
            ..FormatOptions::default()
        };
        print_with(options, "", build)
    }

    #[test]
    fn flat_group_stays_on_one_line() {
        let output = print_document(
            |f| {
                f.group(|grouped| {
                    grouped.token("{");
                    grouped.soft_line_or_space();
                    grouped.token("x");
                    grouped.soft_line_or_space();
                    grouped.token("}");
                });
            },
            80,
        );
        assert_eq!(output, "{ x }");
    }

    #[test]
    fn wide_group_breaks() {
        let long_text = "x".repeat(90);
        let output = print_document(
            |f| {
                let text = f.allocator().alloc_str(&long_text);
                f.group(|grouped| {
                    grouped.token("{");
                    grouped.indent(|indented| {
                        indented.soft_line_or_space();
                        indented.text(text);
                    });
                    grouped.soft_line_or_space();
                    grouped.token("}");
                });
            },
            80,
        );
        assert_eq!(output, format!("{{\n    {long_text}\n}}"));
    }

    #[test]
    fn hard_line_forces_all_enclosing_groups() {
        let output = print_document(
            |f| {
                f.group(|outer| {
                    outer.token("a");
                    outer.group(|inner| {
                        inner.token("(");
                        inner.soft_line();
                        inner.hard_line();
                        inner.token(")");
                    });
                    outer.soft_line_or_space();
                    outer.token("b");
                });
            },
            80,
        );
        // Both the inner and outer group must expand.
        assert_eq!(
            output,
            "\
a(

)
b"
        );
    }

    fn write_conditional_comma(f: &mut Formatter<'_>, content: &str) {
        let id = f.group_id();
        let text = f.allocator().alloc_str(content);
        f.group_with_id(id, |grouped| {
            grouped.token("[");
            grouped.soft_line();
            grouped.text(text);
            grouped.if_group_breaks(id, |broken| broken.token(","));
            grouped.soft_line();
            grouped.token("]");
        });
    }

    #[test]
    fn if_group_breaks_prints_when_its_group_breaks() {
        let content = "y".repeat(100);
        let output = print_document(|f| write_conditional_comma(f, &content), 80);
        assert_eq!(output, format!("[\n{content},\n]"));
    }

    #[test]
    fn if_group_breaks_absent_when_flat() {
        assert_eq!(print_document(|f| write_conditional_comma(f, "y"), 80), "[y]");
    }

    #[test]
    fn trailing_whitespace_is_trimmed() {
        let output = print_document(
            |f| {
                f.token("a");
                f.space();
                f.hard_line();
                f.token("b");
            },
            80,
        );
        assert_eq!(
            output,
            "\
a
b"
        );
    }

    #[test]
    fn blank_lines_do_not_stack() {
        let output = print_document(
            |f| {
                f.token("a");
                f.empty_line();
                f.empty_line();
                f.token("b");
            },
            80,
        );
        assert_eq!(
            output,
            "\
a

b"
        );
    }

    #[test]
    fn line_suffix_defers_to_line_end() {
        let output = print_document(
            |f| {
                f.token("local x = 1");
                f.line_suffix("-- trailing");
                f.hard_line();
                f.token("local y = 2");
            },
            80,
        );
        assert_eq!(
            output,
            "\
local x = 1 -- trailing
local y = 2"
        );
    }

    fn tab_options(indent_width: u8) -> FormatOptions {
        FormatOptions {
            line_width: 10,
            indent_style: IndentStyle::Tabs,
            indent_width,
            ..FormatOptions::default()
        }
    }

    /// Nine columns when flat, so it fits in ten after one more column and
    /// not after four.
    fn write_bracketed(f: &mut Formatter<'_>) {
        f.group(|grouped| {
            grouped.token("[");
            grouped.soft_line();
            grouped.token("yyyyyyy");
            grouped.soft_line();
            grouped.token("]");
        });
    }

    #[test]
    fn tab_indent_measures_indent_width_columns() {
        let print = |indent_width: u8| {
            print_with(tab_options(indent_width), "", |f| {
                f.token("a");
                f.indent(|indented| {
                    indented.hard_line();
                    write_bracketed(indented);
                });
            })
        };
        assert_eq!(
            print(1),
            "\
a
\t[yyyyyyy]"
        );
        assert_eq!(
            print(4),
            "\
a
\t[
\tyyyyyyy
\t]"
        );
    }

    #[test]
    fn tab_in_base_indent_measures_indent_width_columns() {
        let print = |indent_width: u8, base_indent: &str| {
            print_with(tab_options(indent_width), base_indent, write_bracketed)
        };
        assert_eq!(print(1, "\t"), "\t[yyyyyyy]");
        assert_eq!(
            print(4, "\t"),
            "\
\t[
\tyyyyyyy
\t]"
        );
        assert_eq!(print(4, " "), " [yyyyyyy]");
    }

    /// `(a, <hugged>, bbbb)`, laid out as a call lays out its arguments.
    fn write_hugging_list(f: &mut Formatter<'_>, write_hugged: impl FnOnce(&mut Formatter<'_>)) {
        let list = f.group_id();
        f.group_with_id(list, |grouped| {
            grouped.token("(");
            grouped.indent_if_group_breaks(list, |indented| {
                indented.soft_line();
                indented.token("a");
                indented.token(",");
                indented.soft_line_or_space();
                write_hugged(indented);
                indented.token(",");
                indented.soft_line_or_space();
                indented.token("bbbb");
            });
            grouped.soft_line();
            grouped.token(")");
        });
    }

    /// `{ <content> }`, laid out as a table.
    fn write_braces(f: &mut Formatter<'_>, content: &'static str) {
        f.token("{");
        f.indent(|indented| {
            indented.soft_line_or_space();
            indented.token(content);
        });
        f.soft_line_or_space();
        f.token("}");
    }

    /// A block that always breaks: `do`, an indented `x`, and `end`.
    fn write_do_block(f: &mut Formatter<'_>) {
        f.token("do");
        f.indent(|indented| {
            indented.hard_line();
            indented.token("x");
        });
        f.hard_line();
        f.token("end");
    }

    fn write_hugged_braces(f: &mut Formatter<'_>) {
        write_hugging_list(f, |list| {
            let id = list.group_id();
            list.hugged_group_with_id(id, |hugged| write_braces(hugged, "xxxx"));
        });
    }

    #[test]
    fn hugged_group_breaks_in_its_parents_place() {
        assert_eq!(print_document(write_hugged_braces, 19), "(a, { xxxx }, bbbb)");
        assert_eq!(
            print_document(write_hugged_braces, 18),
            "\
(a, {
    xxxx
}, bbbb)"
        );
    }

    #[test]
    fn hugging_needs_its_closing_line_to_fit() {
        // `}, bbbb)` is eight columns.
        assert_eq!(
            print_document(write_hugged_braces, 8),
            "\
(a, {
    xxxx
}, bbbb)"
        );
        assert_eq!(
            print_document(write_hugged_braces, 7),
            "\
(
    a,
    {
        xxxx
    },
    bbbb
)"
        );
    }

    /// `({xxxx}, <block>)`, both hugged.
    fn write_hugged_braces_and(f: &mut Formatter<'_>, write_block: fn(&mut Formatter<'_>)) {
        let (list, braces_group, block_group) = (f.group_id(), f.group_id(), f.group_id());
        f.group_with_id(list, |grouped| {
            grouped.token("(");
            grouped.indent_if_group_breaks(list, |indented| {
                indented.soft_line();
                indented.hugged_group_with_id(braces_group, |hugged| write_braces(hugged, "xxxx"));
                indented.token(",");
                indented.soft_line_or_space();
                indented.hugged_group_with_id(block_group, write_block);
            });
            grouped.soft_line();
            grouped.token(")");
        });
    }

    #[test]
    fn hugged_group_stays_flat_beside_one_a_forced_break_expands() {
        let write = |f: &mut Formatter<'_>| write_hugged_braces_and(f, write_do_block);
        // `({ xxxx }, do` is thirteen columns.
        assert_eq!(
            print_document(write, 13),
            "\
({ xxxx }, do
    x
end)"
        );
        assert_eq!(
            print_document(write, 12),
            "\
({
    xxxx
}, do
    x
end)"
        );
    }

    #[test]
    fn hugged_groups_all_expand_when_none_is_forced_to() {
        let write = |f: &mut Formatter<'_>| {
            write_hugged_braces_and(f, |hugged| write_braces(hugged, "yyyy"));
        };
        assert_eq!(print_document(write, 20), "({ xxxx }, { yyyy })");
        assert_eq!(
            print_document(write, 19),
            "\
({
    xxxx
}, {
    yyyy
})"
        );
    }

    #[test]
    fn forced_break_in_a_hugged_group_leaves_its_parent_flat() {
        let write_hugged_block = |f: &mut Formatter<'_>| {
            write_hugging_list(f, |list| {
                let block = list.group_id();
                list.hugged_group_with_id(block, write_do_block);
            });
        };
        assert_eq!(
            print_document(write_hugged_block, 80),
            "\
(a, do
    x
end, bbbb)"
        );
        // `(a, do` is six columns.
        assert_eq!(
            print_document(write_hugged_block, 5),
            "\
(
    a,
    do
        x
    end,
    bbbb
)"
        );
    }

    /// `(a, (a, {xxxx}, bbbb), bbbb)`, where the inner list is hugged by the
    /// outer one and hugs its own table.
    fn write_nested_hugging_lists(f: &mut Formatter<'_>) {
        write_hugging_list(f, |outer| {
            let inner = outer.group_id();
            outer.hugged_group_with_id(inner, |hugged| {
                hugged.token("(");
                hugged.indent_if_group_breaks(inner, |indented| {
                    indented.soft_line();
                    indented.token("a");
                    indented.token(",");
                    indented.soft_line_or_space();
                    let braces = indented.group_id();
                    indented.hugged_group_with_id(braces, |table| write_braces(table, "xxxx"));
                    indented.token(",");
                    indented.soft_line_or_space();
                    indented.token("bbbb");
                });
                hugged.soft_line();
                hugged.token(")");
            });
        });
    }

    #[test]
    fn a_hugged_group_that_hugs_keeps_its_own_hugs_apart() {
        let print = |line_width| print_document(write_nested_hugging_lists, line_width);
        assert_eq!(print(30), "(a, (a, { xxxx }, bbbb), bbbb)");
        assert_eq!(
            print(29),
            "\
(a, (
    a,
    { xxxx },
    bbbb
), bbbb)"
        );
        assert_eq!(
            print(10),
            "\
(a, (
    a,
    {
        xxxx
    },
    bbbb
), bbbb)"
        );
    }
}
