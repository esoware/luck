//! Two passes.
//!
//! `propagate_expand` first marks every group that contains a
//! forced break (a hard or empty line, `ExpandParent`, or multi-line text) as
//! expanded, since such a group can never print flat and isn't worth
//! measuring. The print loop then walks the stream with a mode stack.
//! Entering an undecided group measures whether its content fits the
//! remaining width and commits to `Flat` or `Expanded`, and each decision is
//! recorded by `GroupId` so conditional content elsewhere can look it up.

use core::mem::take;

use luck_token::CodeBuffer;
use unicode_width::UnicodeWidthStr as _;

use crate::ir::{FormatElement, GroupId, LineMode, Tag};
use crate::{FormatOptions, IndentStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrintMode {
    Flat,
    Expanded,
}

pub(crate) fn print(
    elements: &[FormatElement],
    group_count: usize,
    options: &FormatOptions,
    base_indent: &str,
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
        base_indent: base_indent.to_owned(),
        base_indent_columns,
        // Synthetic ASTs have no source, so size by element count. Four bytes
        // per element is close enough to avoid most reallocations.
        output: CodeBuffer::with_capacity(elements.len().saturating_mul(4)),
        line_start: 0,
        display_column: 0,
        indent_level: 0,
        modes: Vec::new(),
        group_modes: vec![None; group_count],
        suffixes: Vec::new(),
        is_pending_space: false,
        is_pending_indent: !base_indent.is_empty(),
    };
    propagate_expand(elements, &mut printer.group_modes);
    printer.print_elements(elements);
    printer.flush_suffixes();
    printer.output.into_string()
}

#[derive(Debug, Clone, Copy)]
enum Region {
    Group,
    Entry,
}

impl Region {
    const fn opens(self, element: &FormatElement) -> bool {
        match self {
            Self::Group => matches!(element, FormatElement::Tag(Tag::StartGroup(_))),
            Self::Entry => matches!(element, FormatElement::Tag(Tag::StartEntry)),
        }
    }

    const fn closes(self, element: &FormatElement) -> bool {
        match self {
            Self::Group => matches!(element, FormatElement::Tag(Tag::EndGroup)),
            Self::Entry => matches!(element, FormatElement::Tag(Tag::EndEntry)),
        }
    }
}

struct Printer {
    line_width: usize,
    /// One indent level is `bytes_per_indent` copies of `indent_byte` and
    /// measures `columns_per_indent` columns, so a tab is one byte and as wide
    /// as `indent_width` says.
    indent_byte: u8,
    bytes_per_indent: usize,
    columns_per_indent: usize,
    /// Written before the indent of every line.
    base_indent: String,
    /// A tab in `base_indent` measures `columns_per_indent` too.
    base_indent_columns: usize,
    output: CodeBuffer,
    line_start: usize,
    display_column: usize,
    indent_level: usize,
    /// Effective print-mode stack; empty means `Expanded` (top level).
    modes: Vec<PrintMode>,
    /// Break decision per group id. Pre-seeded with `Expanded` for groups
    /// containing forced breaks; filled in as groups are entered.
    group_modes: Vec<Option<PrintMode>>,
    suffixes: Vec<FormatElement>,
    is_pending_space: bool,
    is_pending_indent: bool,
}

/// A forced break marks every group on the stack of enclosing groups. Line
/// suffixes are boundaries: breaks inside them don't force anything outside.
fn propagate_expand(elements: &[FormatElement], group_modes: &mut [Option<PrintMode>]) {
    let mut group_stack: Vec<GroupId> = Vec::new();
    let mut is_in_suffix = false;

    for element in elements {
        match element {
            FormatElement::Tag(Tag::StartGroup(id)) => group_stack.push(*id),
            FormatElement::Tag(Tag::EndGroup) => {
                group_stack.pop();
            }
            FormatElement::Tag(Tag::StartLineSuffix) => is_in_suffix = true,
            FormatElement::Tag(Tag::EndLineSuffix) => is_in_suffix = false,
            FormatElement::Line(LineMode::Hard | LineMode::Empty) | FormatElement::ExpandParent => {
                if !is_in_suffix {
                    mark_expanded(&group_stack, group_modes);
                }
            }
            FormatElement::Text(content) | FormatElement::Verbatim(content) => {
                if !is_in_suffix && content.contains('\n') {
                    mark_expanded(&group_stack, group_modes);
                }
            }
            FormatElement::Token(_)
            | FormatElement::Space
            | FormatElement::Line(LineMode::Soft | LineMode::SoftOrSpace)
            | FormatElement::LineSuffixBoundary
            | FormatElement::Tag(_) => {}
        }
    }
}

fn mark_expanded(group_stack: &[GroupId], group_modes: &mut [Option<PrintMode>]) {
    for id in group_stack {
        if let Some(mode) = group_modes.get_mut(id.index()) {
            *mode = Some(PrintMode::Expanded);
        }
    }
}

impl Printer {
    fn current_mode(&self) -> PrintMode {
        *self.modes.last().unwrap_or(&PrintMode::Expanded)
    }

    fn group_mode(&self, id: GroupId) -> Option<PrintMode> {
        self.group_modes.get(id.index()).copied().flatten()
    }

    fn print_elements(&mut self, elements: &[FormatElement]) {
        let mut remaining = elements;
        while let Some((element, rest)) = remaining.split_first() {
            remaining = self.print_element(element, rest);
        }
    }

    /// Print `element`, returning the elements left after it and after any
    /// region it consumed from `rest`, e.g. a skipped conditional body.
    fn print_element<'elements>(
        &mut self,
        element: &FormatElement,
        rest: &'elements [FormatElement],
    ) -> &'elements [FormatElement] {
        match element {
            FormatElement::Token(content) => self.emit_text(content),
            FormatElement::Text(content) => self.emit_text(content),
            FormatElement::Verbatim(content) => self.emit_verbatim(content),
            FormatElement::Space => self.is_pending_space = true,
            FormatElement::Line(mode) => self.print_line(*mode),
            FormatElement::ExpandParent => {}
            FormatElement::LineSuffixBoundary => {
                if !self.suffixes.is_empty() {
                    self.emit_newline(false);
                }
            }
            FormatElement::Tag(tag) => return self.print_tag(*tag, rest),
        }
        rest
    }

    fn print_tag<'elements>(
        &mut self,
        tag: Tag,
        rest: &'elements [FormatElement],
    ) -> &'elements [FormatElement] {
        match tag {
            Tag::StartGroup(id) => {
                // A mode already set was forced by `propagate_expand`.
                let mode = self.group_mode(id).unwrap_or_else(|| {
                    // An enclosing flat group already measured this content;
                    // re-measuring can't change the answer.
                    if self.current_mode() == PrintMode::Flat || self.fits(rest, Region::Group, 0) {
                        PrintMode::Flat
                    } else {
                        PrintMode::Expanded
                    }
                });
                if let Some(group_mode) = self.group_modes.get_mut(id.index()) {
                    *group_mode = Some(mode);
                }
                self.modes.push(mode);
            }
            Tag::StartFlat => self.modes.push(PrintMode::Flat),
            Tag::EndGroup | Tag::EndFlat => {
                self.modes.pop();
            }
            Tag::StartIndent => self.indent_level = self.indent_level.saturating_add(1),
            Tag::EndIndent => self.indent_level = self.indent_level.saturating_sub(1),
            Tag::StartIfGroupBreaks(group_id) => {
                let decided_mode = self.group_mode(group_id);
                debug_assert!(
                    decided_mode.is_some(),
                    "conditional references group {group_id:?} before its decision"
                );
                if decided_mode != Some(PrintMode::Expanded) {
                    return skip_conditional(rest);
                }
            }
            Tag::StartFill => return self.print_fill(rest),
            Tag::StartLineSuffix => {
                let (body, after_suffix) = split_line_suffix(rest);
                self.suffixes.extend_from_slice(body);
                return after_suffix;
            }
            // A taken conditional's end tag is a no-op. `print_fill` and
            // `split_line_suffix` consume the other tags listed here, so only
            // malformed IR reaches this arm with one of them.
            Tag::EndIfGroupBreaks
            | Tag::EndFill
            | Tag::StartEntry
            | Tag::EndEntry
            | Tag::EndLineSuffix => {}
        }
        rest
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
    fn print_fill<'elements>(
        &mut self,
        elements: &'elements [FormatElement],
    ) -> &'elements [FormatElement] {
        let mut remaining = elements;
        while let Some((element, rest)) = remaining.split_first() {
            match element {
                FormatElement::Tag(Tag::EndFill) => return rest,
                FormatElement::Line(separator) => {
                    debug_assert!(
                        matches!(rest.first(), Some(FormatElement::Tag(Tag::StartEntry))),
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
                FormatElement::Tag(Tag::StartEntry) => {
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
                        && !matches!(content, FormatElement::Tag(Tag::EndEntry))
                    {
                        entry_remaining = self.print_element(content, after_content);
                    }
                    self.modes.pop();
                    remaining = entry_remaining.get(1..).unwrap_or_default();
                }
                FormatElement::Token(_)
                | FormatElement::Text(_)
                | FormatElement::Verbatim(_)
                | FormatElement::Space
                | FormatElement::ExpandParent
                | FormatElement::LineSuffixBoundary
                | FormatElement::Tag(_) => {
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
    fn fits(&self, elements: &[FormatElement], region: Region, separator_width: usize) -> bool {
        let mut used_width = self.occupied_width().saturating_add(separator_width);
        let mut depth: u32 = 1;
        let mut remaining = elements;

        while let Some((element, rest)) = remaining.split_first() {
            remaining = rest;
            if region.opens(element) {
                depth = depth.saturating_add(1);
            }
            if region.closes(element) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return true;
                }
            }
            match element {
                FormatElement::Token(content) => {
                    used_width = used_width.saturating_add(content.width());
                }
                FormatElement::Text(content) | FormatElement::Verbatim(content) => {
                    if content.contains('\n') {
                        return false;
                    }
                    used_width = used_width.saturating_add(content.width());
                }
                FormatElement::Space | FormatElement::Line(LineMode::SoftOrSpace) => {
                    used_width = used_width.saturating_add(1);
                }
                FormatElement::Line(LineMode::Hard | LineMode::Empty)
                | FormatElement::ExpandParent => return false,
                FormatElement::Tag(Tag::StartIfGroupBreaks(group_id)) => {
                    // A decided group answers with its decision. An undecided
                    // one is being measured now, and prints flat if this
                    // measurement succeeds.
                    if self.group_mode(*group_id) != Some(PrintMode::Expanded) {
                        remaining = skip_conditional(rest);
                        continue;
                    }
                }
                FormatElement::Tag(Tag::StartLineSuffix) => {
                    (_, remaining) = split_line_suffix(rest);
                }
                FormatElement::Line(LineMode::Soft)
                | FormatElement::LineSuffixBoundary
                | FormatElement::Tag(_) => {}
            }
            if used_width > self.line_width {
                return false;
            }
        }
        true
    }

    fn emit_text(&mut self, content: &str) {
        if self.is_pending_indent {
            self.flush_indent();
        }
        if self.is_pending_space {
            self.is_pending_space = false;
            self.output.print_ascii_byte(b' ');
            self.display_column = self.display_column.saturating_add(1);
        }
        self.output.print_str(content);
        match content.rsplit_once('\n') {
            Some((_, last_line)) => {
                // Multi-line text: the column restarts from its last line.
                self.line_start = self.output.len().saturating_sub(last_line.len());
                self.display_column = last_line.width();
            }
            None => self.display_column = self.display_column.saturating_add(content.width()),
        }
    }

    /// Copy source text, which brings its own indentation and keeps any
    /// trailing whitespace. Text ending in a line break leaves the next line to
    /// the printer's indent.
    fn emit_verbatim(&mut self, content: &str) {
        self.is_pending_indent = false;
        self.emit_text(content);
        self.line_start = self.output.len();
        self.is_pending_indent = content.ends_with(['\n', '\r']);
    }

    fn emit_newline(&mut self, is_blank_line: bool) {
        self.flush_suffixes();
        self.is_pending_space = false;
        self.trim_trailing_whitespace();
        // Blank lines never stack, so a break right after one adds nothing.
        while !self.output.as_bytes().ends_with(b"\n\n") {
            self.output.print_ascii_byte(b'\n');
            if !is_blank_line {
                break;
            }
        }
        self.line_start = self.output.len();
        self.display_column = 0;
        self.is_pending_indent = true;
    }

    fn flush_suffixes(&mut self) {
        if self.suffixes.is_empty() {
            return;
        }
        let mut suffixes = take(&mut self.suffixes);
        for element in &suffixes {
            match element {
                FormatElement::Token(content) => self.emit_text(content),
                FormatElement::Text(content) => self.emit_text(content),
                FormatElement::Space => self.is_pending_space = true,
                // `Formatter::write_line_suffix` writes only a space and
                // text, so nothing structural can reach a suffix body.
                FormatElement::Verbatim(_)
                | FormatElement::Line(_)
                | FormatElement::ExpandParent
                | FormatElement::LineSuffixBoundary
                | FormatElement::Tag(_) => {
                    debug_assert!(false, "unsupported element in line suffix: {element:?}");
                }
            }
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
        self.output.print_str(&self.base_indent);
        let indent_bytes = self.indent_level.saturating_mul(self.bytes_per_indent);
        self.output.print_ascii_repeat(self.indent_byte, indent_bytes);
        self.display_column = self.display_column.saturating_add(self.indent_columns());
    }
}

const fn is_start_conditional(element: &FormatElement) -> bool {
    matches!(element, FormatElement::Tag(Tag::StartIfGroupBreaks(_)))
}

const fn is_end_conditional(element: &FormatElement) -> bool {
    matches!(element, FormatElement::Tag(Tag::EndIfGroupBreaks))
}

const fn is_end_suffix(element: &FormatElement) -> bool {
    matches!(element, FormatElement::Tag(Tag::EndLineSuffix))
}

fn skip_conditional(elements: &[FormatElement]) -> &[FormatElement] {
    let mut remaining = elements;
    while let Some((element, rest)) = remaining.split_first() {
        if is_end_conditional(element) {
            return rest;
        }
        remaining = if is_start_conditional(element) { skip_conditional(rest) } else { rest };
    }
    remaining
}

/// Split the elements after a `StartLineSuffix` into the suffix body and what
/// follows its end tag. A body is comment text, so suffixes never nest.
fn split_line_suffix(elements: &[FormatElement]) -> (&[FormatElement], &[FormatElement]) {
    let mut sections = elements.splitn(2, is_end_suffix);
    let body = sections.next().unwrap_or_default();
    (body, sections.next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use crate::comments::Comments;
    use crate::ir::{
        Format as _, Formatter, empty_line, format_with, group, group_with_id, hard_line,
        if_group_breaks, indent, soft_line, soft_line_or_space, space, token,
    };
    use crate::{FormatOptions, IndentStyle};

    fn print_document(build: impl Fn(&mut Formatter), line_width: u16) -> String {
        let options = FormatOptions {
            line_width,
            indent_style: IndentStyle::Spaces,
            indent_width: 4,
            ..FormatOptions::default()
        };
        let mut f = Formatter::new(options, Comments::none());
        build(&mut f);
        f.print("")
    }

    #[test]
    fn flat_group_stays_on_one_line() {
        let output = print_document(
            |f| {
                group((
                    token("{"),
                    soft_line_or_space(),
                    token("x"),
                    soft_line_or_space(),
                    token("}"),
                ))
                .fmt(f);
            },
            80,
        );
        assert_eq!(output, "{ x }");
    }

    #[test]
    fn wide_group_breaks() {
        let long_text = "x".repeat(90);
        let output = print_document(
            move |f| {
                group((
                    token("{"),
                    indent((
                        soft_line_or_space(),
                        format_with(|indented| indented.write_text(long_text.clone())),
                    )),
                    soft_line_or_space(),
                    token("}"),
                ))
                .fmt(f);
            },
            80,
        );
        assert!(output.starts_with("{\n    x"));
        assert!(output.ends_with("\n}"));
    }

    #[test]
    fn hard_line_forces_all_enclosing_groups() {
        let output = print_document(
            |f| {
                group((
                    token("a"),
                    group((token("("), soft_line(), hard_line(), token(")"))),
                    soft_line_or_space(),
                    token("b"),
                ))
                .fmt(f);
            },
            80,
        );
        // Both the inner and outer group must expand.
        assert_eq!(output, "a(\n\n)\nb");
    }

    #[test]
    fn if_group_breaks_prints_when_its_group_breaks() {
        let output = print_document(
            |f| {
                let id = f.group_id();
                group_with_id(
                    id,
                    (
                        token("["),
                        soft_line(),
                        format_with(|grouped| grouped.write_text("y".repeat(100))),
                        if_group_breaks(id, token(",")),
                        soft_line(),
                        token("]"),
                    ),
                )
                .fmt(f);
            },
            80,
        );
        assert!(output.contains(','), "trailing comma appears when broken");
    }

    #[test]
    fn if_group_breaks_absent_when_flat() {
        let output = print_document(
            |f| {
                let id = f.group_id();
                group_with_id(
                    id,
                    (
                        token("["),
                        soft_line(),
                        token("y"),
                        if_group_breaks(id, token(",")),
                        soft_line(),
                        token("]"),
                    ),
                )
                .fmt(f);
            },
            80,
        );
        assert_eq!(output, "[y]");
    }

    #[test]
    fn trailing_whitespace_is_trimmed() {
        let output = print_document(
            |f| {
                (token("a"), space(), hard_line(), token("b")).fmt(f);
            },
            80,
        );
        assert_eq!(output, "a\nb");
    }

    #[test]
    fn blank_lines_do_not_stack() {
        let output = print_document(
            |f| {
                (token("a"), empty_line(), empty_line(), token("b")).fmt(f);
            },
            80,
        );
        assert_eq!(output, "a\n\nb");
    }

    #[test]
    fn line_suffix_defers_to_line_end() {
        let output = print_document(
            |f| {
                token("local x = 1").fmt(f);
                f.write_line_suffix("-- trailing".into());
                (hard_line(), token("local y = 2")).fmt(f);
            },
            80,
        );
        assert_eq!(output, "local x = 1 -- trailing\nlocal y = 2");
    }

    fn tab_formatter(indent_width: u8) -> Formatter {
        let options = FormatOptions {
            line_width: 10,
            indent_style: IndentStyle::Tabs,
            indent_width,
            ..FormatOptions::default()
        };
        Formatter::new(options, Comments::none())
    }

    /// Nine columns when flat, so it fits in ten after one more column and
    /// not after four.
    fn write_bracketed(f: &mut Formatter) {
        group((token("["), soft_line(), token("yyyyyyy"), soft_line(), token("]"))).fmt(f);
    }

    #[test]
    fn tab_indent_measures_indent_width_columns() {
        let print = |indent_width: u8| {
            let mut f = tab_formatter(indent_width);
            (token("a"), indent((hard_line(), format_with(write_bracketed)))).fmt(&mut f);
            f.print("")
        };
        assert_eq!(print(1), "a\n\t[yyyyyyy]");
        assert_eq!(print(4), "a\n\t[\n\tyyyyyyy\n\t]");
    }

    #[test]
    fn tab_in_base_indent_measures_indent_width_columns() {
        let print = |indent_width: u8, base_indent: &str| {
            let mut f = tab_formatter(indent_width);
            write_bracketed(&mut f);
            f.print(base_indent)
        };
        assert_eq!(print(1, "\t"), "\t[yyyyyyy]");
        assert_eq!(print(4, "\t"), "\t[\n\tyyyyyyy\n\t]");
        assert_eq!(print(4, " "), " [yyyyyyy]");
    }
}
