//! IR -> text layout.
//!
//! Two passes. `propagate_expand` first marks every group that contains a
//! forced break (hard/empty line, `ExpandParent`, multi-line text) - such a
//! group can never print flat, so measuring it would be wasted work and,
//! worse, `fits` would have to re-discover the break each time. The print
//! loop then walks the stream with a mode stack: entering an unforced group
//! measures whether its content fits the remaining width and commits to
//! `Flat` or `Expanded`; every decision is recorded per `GroupId` so
//! conditional content elsewhere can query it.

use luck_token::code_buffer::CodeBuffer;
use unicode_width::UnicodeWidthStr;

use crate::ir::{FormatElement, GroupId, LineMode, PrintMode, Tag};

/// Layout knobs the printer needs; the caller maps `FormatOptions` down to
/// this so the printer stays decoupled from config types.
#[derive(Debug, Clone, Copy)]
pub struct PrinterOptions {
    pub line_width: u16,
    pub use_tabs: bool,
    pub indent_width: u8,
}

/// Lay out a document. `group_count` is `Formatter::group_count()`.
pub fn print(elements: &[FormatElement], group_count: u32, options: &PrinterOptions) -> String {
    let mut printer = Printer {
        options: *options,
        // Element count is the only size signal available here (synthetic
        // ASTs have no source); a few output bytes per element is a close
        // enough hint to avoid most realloc copies.
        output: CodeBuffer::with_capacity(elements.len() * 4),
        line_start: 0,
        column: 0,
        indents: Vec::new(),
        modes: Vec::new(),
        group_modes: vec![None; group_count as usize],
        suffixes: Vec::new(),
        is_pending_space: false,
        is_pending_indent: false,
    };
    propagate_expand(elements, &mut printer.group_modes);
    printer.print_elements(elements);
    printer.flush_suffixes();
    printer.output.into_string()
}

/// One entry on the indent stack. `None` keeps push/pop symmetric for
/// conditional indents that resolved to "no indent".
#[derive(Debug, Clone, Copy)]
enum Indent {
    Level,
    Align(u8),
    Inactive,
}

struct Printer {
    options: PrinterOptions,
    output: CodeBuffer,
    /// Byte offset in `output` where the current line begins.
    line_start: usize,
    /// Display width of the current line so far.
    column: usize,
    indents: Vec<Indent>,
    /// Effective print-mode stack; empty means `Expanded` (top level).
    modes: Vec<PrintMode>,
    /// Break decision per group id. Pre-seeded with `Expanded` for groups
    /// containing forced breaks; filled in as groups are entered.
    group_modes: Vec<Option<PrintMode>>,
    /// Queued line-suffix elements, flushed before the next newline.
    suffixes: Vec<FormatElement>,
    is_pending_space: bool,
    is_pending_indent: bool,
}

/// Mark groups that contain a forced break as `Expanded` before printing.
/// A stack of enclosing group ids is maintained; a forced break marks every
/// group on it. `BestFitting` variants and line suffixes are boundaries -
/// breaks inside them don't force the outside.
fn propagate_expand(elements: &[FormatElement], group_modes: &mut [Option<PrintMode>]) {
    let mut group_stack: Vec<GroupId> = Vec::new();
    let mut suffix_depth: u32 = 0;

    for element in elements {
        match element {
            FormatElement::Tag(Tag::StartGroup(id)) => group_stack.push(*id),
            FormatElement::Tag(Tag::EndGroup) => {
                group_stack.pop();
            }
            FormatElement::Tag(Tag::StartLineSuffix) => suffix_depth += 1,
            FormatElement::Tag(Tag::EndLineSuffix) => suffix_depth -= 1,
            FormatElement::Line(LineMode::Hard | LineMode::Empty) | FormatElement::ExpandParent => {
                if suffix_depth == 0 {
                    for id in &group_stack {
                        group_modes[id.0 as usize] = Some(PrintMode::Expanded);
                    }
                }
            }
            FormatElement::Text(content) => {
                if suffix_depth == 0 && content.contains('\n') {
                    for id in &group_stack {
                        group_modes[id.0 as usize] = Some(PrintMode::Expanded);
                    }
                }
            }
            // Boundary: variants keep their breaks to themselves, but the
            // groups inside each variant still need their own marks.
            FormatElement::BestFitting(variants) => {
                for variant in variants {
                    propagate_expand(variant, group_modes);
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

impl Printer {
    fn current_mode(&self) -> PrintMode {
        *self.modes.last().unwrap_or(&PrintMode::Expanded)
    }

    fn print_elements(&mut self, elements: &[FormatElement]) {
        let mut index = 0;
        while index < elements.len() {
            index = self.print_element(elements, index);
        }
    }

    /// Print the element at `index`, returning the index after it (and after
    /// any region it consumed, e.g. a skipped conditional body).
    fn print_element(&mut self, elements: &[FormatElement], index: usize) -> usize {
        match &elements[index] {
            FormatElement::Token(content) => {
                self.emit_text(content);
                index + 1
            }
            FormatElement::Text(content) => {
                self.emit_text(content);
                index + 1
            }
            FormatElement::Space => {
                self.is_pending_space = true;
                index + 1
            }
            FormatElement::Line(mode) => {
                self.print_line(*mode);
                index + 1
            }
            FormatElement::ExpandParent => index + 1,
            FormatElement::LineSuffixBoundary => {
                if !self.suffixes.is_empty() {
                    self.emit_newline(false);
                }
                index + 1
            }
            FormatElement::BestFitting(variants) => {
                self.print_best_fitting(variants);
                index + 1
            }
            FormatElement::Tag(tag) => self.print_tag(elements, index, tag),
        }
    }

    fn print_tag(&mut self, elements: &[FormatElement], index: usize, tag: &Tag) -> usize {
        match tag {
            Tag::StartGroup(id) => {
                let mode = match self.group_modes[id.0 as usize] {
                    // Pre-forced by propagate_expand
                    Some(mode) => mode,
                    None => {
                        if self.current_mode() == PrintMode::Flat {
                            // An enclosing flat group already measured this
                            // content; re-measuring can't change the answer.
                            PrintMode::Flat
                        } else if self.fits_region(elements, index + 1, region_end_group) {
                            PrintMode::Flat
                        } else {
                            PrintMode::Expanded
                        }
                    }
                };
                self.group_modes[id.0 as usize] = Some(mode);
                self.modes.push(mode);
                index + 1
            }
            Tag::EndGroup => {
                self.modes.pop();
                index + 1
            }
            Tag::StartIndent => {
                self.indents.push(Indent::Level);
                index + 1
            }
            Tag::EndIndent | Tag::EndAlign | Tag::EndIndentIfGroupBreaks => {
                self.indents.pop();
                index + 1
            }
            Tag::StartAlign(width) => {
                self.indents.push(Indent::Align(*width));
                index + 1
            }
            Tag::StartIndentIfGroupBreaks(id) => {
                let indent = match self.group_modes[id.0 as usize] {
                    Some(PrintMode::Expanded) => Indent::Level,
                    Some(PrintMode::Flat) | None => Indent::Inactive,
                };
                self.indents.push(indent);
                index + 1
            }
            Tag::StartConditional { mode, group_id } => {
                let decided = self.group_modes[group_id.0 as usize];
                debug_assert!(
                    decided.is_some(),
                    "conditional references group {group_id:?} before its decision"
                );
                if decided == Some(*mode) {
                    index + 1
                } else {
                    skip_region(
                        elements,
                        index + 1,
                        is_start_conditional,
                        is_end_conditional,
                    )
                }
            }
            Tag::EndConditional => index + 1,
            Tag::StartFill => self.print_fill(elements, index + 1),
            // Reached only for empty fills or as print_fill's return point
            Tag::EndFill => index + 1,
            // Entries outside print_fill (defensive) print transparently
            Tag::StartEntry | Tag::EndEntry => index + 1,
            Tag::StartLineSuffix => {
                let end = skip_region(elements, index + 1, is_start_suffix, is_end_suffix);
                // `end` is one past EndLineSuffix; queue the body
                self.suffixes
                    .extend_from_slice(&elements[index + 1..end - 1]);
                end
            }
            Tag::EndLineSuffix => index + 1,
            Tag::StartLabelled(_) | Tag::EndLabelled => index + 1,
        }
    }

    fn print_line(&mut self, mode: LineMode) {
        match (self.current_mode(), mode) {
            (PrintMode::Flat, LineMode::Soft) => {}
            (PrintMode::Flat, LineMode::SoftOrSpace) => self.is_pending_space = true,
            // Forced breaks inside a measured-flat region can't happen -
            // propagation expands every enclosing group - but stay safe.
            (_, LineMode::Hard) | (PrintMode::Expanded, LineMode::Soft | LineMode::SoftOrSpace) => {
                self.emit_newline(false);
            }
            (_, LineMode::Empty) => self.emit_newline(true),
        }
    }

    fn print_best_fitting(&mut self, variants: &[Vec<FormatElement>]) {
        for (index, variant) in variants.iter().enumerate() {
            let is_last = index == variants.len() - 1;
            if is_last {
                self.modes.push(PrintMode::Expanded);
                self.print_elements(variant);
                self.modes.pop();
                return;
            }
            if self.fits_region(variant, 0, region_whole) {
                self.modes.push(PrintMode::Flat);
                self.print_elements(variant);
                self.modes.pop();
                return;
            }
        }
    }

    /// Greedy fill: each entry goes on the current line if it fits,
    /// otherwise the preceding separator becomes a newline. Entries that
    /// don't fit even on a fresh line print expanded.
    fn print_fill(&mut self, elements: &[FormatElement], mut index: usize) -> usize {
        loop {
            match &elements[index] {
                FormatElement::Tag(Tag::EndFill) => return index + 1,
                FormatElement::Line(separator) => {
                    // Peek the next entry's flat width to decide the break
                    let entry_start = index + 1;
                    debug_assert!(matches!(
                        elements.get(entry_start),
                        Some(FormatElement::Tag(Tag::StartEntry))
                    ));
                    let separator_width = match separator {
                        LineMode::SoftOrSpace => 1,
                        LineMode::Soft => 0,
                        // Hard separators always break
                        LineMode::Hard | LineMode::Empty => {
                            self.emit_newline(matches!(separator, LineMode::Empty));
                            index = entry_start;
                            continue;
                        }
                    };
                    if self.fits_region_with_extra(
                        elements,
                        entry_start + 1,
                        region_end_entry,
                        separator_width,
                    ) {
                        if matches!(separator, LineMode::SoftOrSpace) {
                            self.is_pending_space = true;
                        }
                    } else {
                        self.emit_newline(false);
                    }
                    index = entry_start;
                }
                FormatElement::Tag(Tag::StartEntry) => {
                    let mode = if self.fits_region(elements, index + 1, region_end_entry) {
                        PrintMode::Flat
                    } else {
                        PrintMode::Expanded
                    };
                    self.modes.push(mode);
                    index += 1;
                    // Entry content prints through `print_element`, not this
                    // loop's arms: a grouped chain inside an entry carries its
                    // own `Line` elements, and the `Line` arm above must only
                    // ever see the separators between entries. A nested fill
                    // consumes its own entries via recursion, so the first
                    // `EndEntry` seen here is this entry's.
                    while !matches!(&elements[index], FormatElement::Tag(Tag::EndEntry)) {
                        index = self.print_element(elements, index);
                    }
                    self.modes.pop();
                    index += 1;
                }
                _ => {
                    index = self.print_element(elements, index);
                }
            }
        }
    }

    fn remaining_width(&self) -> isize {
        // A pending indent hasn't reached `column` yet but will occupy the
        // line before any measured content does.
        let pending_indent = if self.is_pending_indent {
            self.pending_indent_width()
        } else {
            0
        };
        self.options.line_width as isize
            - self.column as isize
            - pending_indent as isize
            - if self.is_pending_space { 1 } else { 0 }
    }

    fn pending_indent_width(&self) -> usize {
        self.indents
            .iter()
            .map(|indent| match indent {
                // Tabs occupy one column in this printer's width model
                Indent::Level if self.options.use_tabs => 1,
                Indent::Level => self.options.indent_width as usize,
                Indent::Align(spaces) => *spaces as usize,
                Indent::Inactive => 0,
            })
            .sum()
    }

    fn fits_region(
        &self,
        elements: &[FormatElement],
        start: usize,
        region: fn(&FormatElement, &mut i32) -> bool,
    ) -> bool {
        self.fits_region_with_extra(elements, start, region, 0)
    }

    /// Simulate flat printing from `start` until `region` reports the end,
    /// checking the content fits the remaining width. `extra` pre-charges
    /// width (a pending separator).
    fn fits_region_with_extra(
        &self,
        elements: &[FormatElement],
        start: usize,
        region: fn(&FormatElement, &mut i32) -> bool,
        extra: isize,
    ) -> bool {
        let mut budget = self.remaining_width() - extra;
        let mut depth: i32 = 1;
        let mut suffix_depth: u32 = 0;
        let mut index = start;

        while index < elements.len() {
            let element = &elements[index];
            if region(element, &mut depth) && depth == 0 {
                return true;
            }
            if suffix_depth == 0 {
                match element {
                    FormatElement::Token(content) => budget -= content.width() as isize,
                    FormatElement::Text(content) => {
                        if content.contains('\n') {
                            return false;
                        }
                        budget -= content.width() as isize;
                    }
                    FormatElement::Space => budget -= 1,
                    FormatElement::Line(LineMode::Soft) => {}
                    FormatElement::Line(LineMode::SoftOrSpace) => budget -= 1,
                    FormatElement::Line(LineMode::Hard | LineMode::Empty)
                    | FormatElement::ExpandParent => return false,
                    FormatElement::LineSuffixBoundary => {}
                    // Measure by the most-flat variant
                    FormatElement::BestFitting(variants) => {
                        if let Some(most_flat) = variants.first() {
                            if !self.fits_slice_flat(most_flat, &mut budget) {
                                return false;
                            }
                        }
                    }
                    FormatElement::Tag(Tag::StartConditional { mode, group_id }) => {
                        // Decided groups answer truthfully; the group being
                        // measured right now would print flat if this
                        // measurement succeeds.
                        let effective =
                            self.group_modes[group_id.0 as usize].unwrap_or(PrintMode::Flat);
                        if effective != *mode {
                            index = skip_region(
                                elements,
                                index + 1,
                                is_start_conditional,
                                is_end_conditional,
                            );
                            continue;
                        }
                    }
                    FormatElement::Tag(_) => {}
                }
            } else if matches!(element, FormatElement::Tag(Tag::EndLineSuffix)) {
                suffix_depth -= 1;
            }
            if matches!(element, FormatElement::Tag(Tag::StartLineSuffix)) {
                suffix_depth += 1;
            }
            if budget < 0 {
                return false;
            }
            index += 1;
        }
        // Ran off the end of the document while under budget
        true
    }

    /// Width-only check of a detached slice (BestFitting variant).
    fn fits_slice_flat(&self, elements: &[FormatElement], budget: &mut isize) -> bool {
        for element in elements {
            match element {
                FormatElement::Token(content) => *budget -= content.width() as isize,
                FormatElement::Text(content) => {
                    if content.contains('\n') {
                        return false;
                    }
                    *budget -= content.width() as isize;
                }
                FormatElement::Space | FormatElement::Line(LineMode::SoftOrSpace) => *budget -= 1,
                FormatElement::Line(LineMode::Soft) => {}
                FormatElement::Line(LineMode::Hard | LineMode::Empty)
                | FormatElement::ExpandParent => return false,
                FormatElement::LineSuffixBoundary => {}
                FormatElement::BestFitting(variants) => {
                    if let Some(most_flat) = variants.first() {
                        if !self.fits_slice_flat(most_flat, budget) {
                            return false;
                        }
                    }
                }
                FormatElement::Tag(_) => {}
            }
            if *budget < 0 {
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
            self.column += 1;
        }
        self.output.print_str(content);
        match content.rfind('\n') {
            Some(last_newline) => {
                // Multi-line text: column restarts after its last line
                self.line_start = self.output.len() - (content.len() - last_newline - 1);
                self.column = content[last_newline + 1..].width();
            }
            None => self.column += content.width(),
        }
    }

    fn emit_newline(&mut self, is_blank_line: bool) {
        self.flush_suffixes();
        self.is_pending_space = false;
        self.trim_trailing_whitespace();
        if is_blank_line {
            // Exactly one blank line, never a stack of them
            while !self.output.as_bytes().ends_with(b"\n\n") {
                self.output.print_ascii_byte(b'\n');
            }
        } else if !self.output.as_bytes().ends_with(b"\n\n") {
            // A hard break directly after a blank line folds into it
            self.output.print_ascii_byte(b'\n');
        }
        self.line_start = self.output.len();
        self.column = 0;
        self.is_pending_indent = true;
    }

    fn flush_suffixes(&mut self) {
        if self.suffixes.is_empty() {
            return;
        }
        let suffixes = std::mem::take(&mut self.suffixes);
        for element in &suffixes {
            match element {
                FormatElement::Token(content) => self.emit_text(content),
                FormatElement::Text(content) => self.emit_text(content),
                FormatElement::Space => self.is_pending_space = true,
                // Suffix bodies are comment text; anything structural
                // inside one is an emitter bug.
                _ => debug_assert!(false, "unsupported element in line suffix: {element:?}"),
            }
        }
    }

    fn trim_trailing_whitespace(&mut self) {
        let bytes = self.output.as_bytes();
        let mut trimmed_end = bytes.len();
        while trimmed_end > self.line_start && matches!(bytes[trimmed_end - 1], b' ' | b'\t') {
            trimmed_end -= 1;
        }
        self.output.truncate(trimmed_end);
    }

    fn flush_indent(&mut self) {
        self.is_pending_indent = false;
        let mut width = 0usize;
        for indent in &self.indents {
            match indent {
                Indent::Level => width += 1,
                Indent::Align(_) | Indent::Inactive => {}
            }
        }
        let align_extra: usize = self
            .indents
            .iter()
            .map(|indent| match indent {
                Indent::Align(spaces) => *spaces as usize,
                Indent::Level | Indent::Inactive => 0,
            })
            .sum();

        if self.options.use_tabs {
            self.output.print_ascii_repeat(b'\t', width);
            // Tabs count as one column here; the printer's width model
            // treats a tab as a single cell, matching the old printer.
            self.column += width;
        } else {
            let spaces = width * self.options.indent_width as usize;
            self.output.print_ascii_repeat(b' ', spaces);
            self.column += spaces;
        }
        self.output.print_ascii_repeat(b' ', align_extra);
        self.column += align_extra;
    }
}

/// Region tracker for group content: depth +/-1 on group tags, end at 0.
fn region_end_group(element: &FormatElement, depth: &mut i32) -> bool {
    match element {
        FormatElement::Tag(Tag::StartGroup(_)) => {
            *depth += 1;
            false
        }
        FormatElement::Tag(Tag::EndGroup) => {
            *depth -= 1;
            true
        }
        _ => false,
    }
}

fn region_end_entry(element: &FormatElement, depth: &mut i32) -> bool {
    match element {
        FormatElement::Tag(Tag::StartEntry) => {
            *depth += 1;
            false
        }
        FormatElement::Tag(Tag::EndEntry) => {
            *depth -= 1;
            true
        }
        _ => false,
    }
}

/// Whole-slice measurement: never ends early.
fn region_whole(_element: &FormatElement, _depth: &mut i32) -> bool {
    false
}

fn is_start_conditional(element: &FormatElement) -> bool {
    matches!(element, FormatElement::Tag(Tag::StartConditional { .. }))
}

fn is_end_conditional(element: &FormatElement) -> bool {
    matches!(element, FormatElement::Tag(Tag::EndConditional))
}

fn is_start_suffix(element: &FormatElement) -> bool {
    matches!(element, FormatElement::Tag(Tag::StartLineSuffix))
}

fn is_end_suffix(element: &FormatElement) -> bool {
    matches!(element, FormatElement::Tag(Tag::EndLineSuffix))
}

/// Skip past a nested region: from `start` (just after the opening tag) to
/// one past the matching close.
fn skip_region(
    elements: &[FormatElement],
    start: usize,
    is_open: fn(&FormatElement) -> bool,
    is_close: fn(&FormatElement) -> bool,
) -> usize {
    let mut depth = 1;
    let mut index = start;
    while index < elements.len() {
        let element = &elements[index];
        if is_open(element) {
            depth += 1;
        } else if is_close(element) {
            depth -= 1;
            if depth == 0 {
                return index + 1;
            }
        }
        index += 1;
    }
    elements.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        Format, Formatter, best_fitting, group, group_with_id, hard_line, if_group_breaks, indent,
        soft_line, soft_line_or_space, space, text, token,
    };

    fn print_document(build: impl Fn(&mut Formatter), line_width: u16) -> String {
        let mut formatter = Formatter::new();
        build(&mut formatter);
        let group_count = formatter.group_count();
        let elements = formatter.into_elements();
        print(
            &elements,
            group_count,
            &PrinterOptions {
                line_width,
                use_tabs: false,
                indent_width: 4,
            },
        )
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
        let long = "x".repeat(90);
        let output = print_document(
            move |f| {
                group((
                    token("{"),
                    indent((soft_line_or_space(), text(long.clone()))),
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
        // Both the inner and outer group must expand
        assert_eq!(output, "a(\n\n)\nb");
    }

    #[test]
    fn if_group_breaks_by_id() {
        let output = print_document(
            |f| {
                let id = f.group_id();
                group_with_id(
                    id,
                    (
                        token("["),
                        soft_line(),
                        text("y".repeat(100)),
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
    fn best_fitting_picks_first_that_fits() {
        let output = print_document(
            |f| {
                token("local x = ").fmt(f);
                best_fitting(&[
                    &(token("aaaa"), space(), token("bbbb")) as &dyn Format,
                    &(token("aaaa"), hard_line(), token("bbbb")) as &dyn Format,
                ])
                .fmt(f);
            },
            80,
        );
        assert_eq!(output, "local x = aaaa bbbb");
    }

    #[test]
    fn best_fitting_falls_back_to_last() {
        let output = print_document(
            |f| {
                text("p".repeat(78)).fmt(f);
                best_fitting(&[
                    &(token("aaaa"), space(), token("bbbb")) as &dyn Format,
                    &(hard_line(), token("bbbb")) as &dyn Format,
                ])
                .fmt(f);
            },
            80,
        );
        assert!(output.ends_with("\nbbbb"));
    }

    #[test]
    fn trailing_whitespace_is_trimmed() {
        let output = print_document(
            |f| {
                token("a").fmt(f);
                space().fmt(f);
                hard_line().fmt(f);
                token("b").fmt(f);
            },
            80,
        );
        assert_eq!(output, "a\nb");
    }

    #[test]
    fn blank_lines_do_not_stack() {
        let output = print_document(
            |f| {
                token("a").fmt(f);
                crate::ir::empty_line().fmt(f);
                crate::ir::empty_line().fmt(f);
                token("b").fmt(f);
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
                crate::ir::line_suffix((space(), text("-- trailing"))).fmt(f);
                hard_line().fmt(f);
                token("local y = 2").fmt(f);
            },
            80,
        );
        assert_eq!(output, "local x = 1 -- trailing\nlocal y = 2");
    }
}
