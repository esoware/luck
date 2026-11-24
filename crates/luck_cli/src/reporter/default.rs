use std::iter;
use std::ops::Range;

use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};
use luck_diagnostics::{Diagnostic, Severity};
use luck_token::{LuaVersion, Span, chunk_start};

use super::{Reporter, SourceFile};

/// Above this line length, ariadne's caret rendering effectively hangs.
///
/// A minified or obfuscated file is one multi-megabyte line, so such
/// diagnostics get a compact, snippet-free rendering instead.
const MAX_RENDERABLE_LINE_CHARS: usize = 10_000;

/// ariadne's report, with a snippet of the code each place a diagnostic
/// points at, for people to read on stderr.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DefaultReporter;

impl Reporter for DefaultReporter {
    fn render(&self, file: SourceFile<'_>, diags: &[Diagnostic], output: &mut Vec<u8>) {
        let Some(text) = file.text else {
            for diag in diags {
                write_compact(file.label, diag, output);
            }
            return;
        };
        let snippet = Snippet::new(text, file.version);
        let is_too_long = |span: Span| {
            let line = snippet.source.get_byte_line(snippet.shifted(span).start);
            line.is_some_and(|(found, _, _)| found.len() > MAX_RENDERABLE_LINE_CHARS)
        };
        for diag in diags {
            let Some(span) = diag.span else {
                write_compact(file.label, diag, output);
                continue;
            };
            if is_too_long(span) || diag.labels.iter().any(|label| is_too_long(label.span)) {
                write_compact(file.label, diag, output);
                continue;
            }
            let cache = (file.label, &snippet.source);
            // A failed write has nowhere to be reported.
            let _ = report(file.label, diag, span, &snippet).write(cache, &mut *output);
        }
    }
}

/// A file's text as ariadne reads it.
struct Snippet {
    source: Source<String>,
    /// How far spans shift to reach `source`, which drops a BOM the file's
    /// dialect skips.
    span_shift: usize,
}

impl Snippet {
    fn new(text: &[u8], version: LuaVersion) -> Self {
        let span_shift = chunk_start(text, version);
        Self { source: Source::from(renderable(&text[span_shift..], version)), span_shift }
    }

    fn shifted(&self, span: Span) -> Range<usize> {
        let Range { start, end } = Range::<usize>::from(span);
        start.saturating_sub(self.span_shift)..end.saturating_sub(self.span_shift)
    }
}

/// `bytes` as text ariadne renders at the same byte offsets.
///
/// Each byte that isn't UTF-8 is a `?`, one column as a one-byte encoding
/// shows it, and only `version`'s line breaks still end a line, since
/// ariadne's own set adds VT, FF, NEL, LS, and PS and counts LFCR as two.
fn renderable(bytes: &[u8], version: LuaVersion) -> String {
    let mut rendered = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        let text = chunk.valid();
        let text_bytes = text.as_bytes();
        let mut copied_to = 0;
        let mut index = 0;
        while let Some(&b) = text_bytes.get(index) {
            let next = |ahead: usize| text_bytes.get(index + ahead).copied();
            let replacement = match b {
                0x0B | 0x0C => " ",
                // NEL
                0xC2 if next(1) == Some(0x85) => "  ",
                // LS and PS
                0xE2 if next(1) == Some(0x80) && matches!(next(2), Some(0xA8 | 0xA9)) => "   ",
                // Luau
                b'\r' if !version.has_lfcr_line_break() && next(1) != Some(b'\n') => " ",
                b'\n' if version.has_lfcr_line_break() && next(1) == Some(b'\r') => "\r\n",
                _ => {
                    index += 1;
                    continue;
                }
            };
            rendered.push_str(&text[copied_to..index]);
            rendered.push_str(replacement);
            index += replacement.len();
            copied_to = index;
        }
        rendered.push_str(&text[copied_to..]);
        rendered.extend(iter::repeat_n('?', chunk.invalid().len()));
    }
    rendered
}

/// The label at `span`, where the problem is, repeats the message, and each
/// other label says what it says of its own place.
fn report<'label>(
    label: &'label str,
    diag: &Diagnostic,
    span: Span,
    snippet: &Snippet,
) -> Report<'label, (&'label str, Range<usize>)> {
    let kind = match diag.severity {
        Severity::Error => ReportKind::Error,
        Severity::Warning => ReportKind::Warning,
    };
    let primary = (label, snippet.shifted(span));
    let mut builder = Report::build(kind, primary.clone())
        .with_code(diag.code)
        .with_message(&diag.message)
        .with_config(Config::default().with_index_type(IndexType::Byte))
        .with_label(Label::new(primary).with_message(&diag.message));
    for (order, other) in (1..).zip(&diag.labels) {
        let place = (label, snippet.shifted(other.span));
        builder =
            builder.with_label(Label::new(place).with_message(&other.message).with_order(order));
    }
    if let Some(help) = &diag.help {
        builder = builder.with_help(help);
    }
    if let Some(note) = &diag.note {
        builder = builder.with_note(note);
    }
    builder.finish()
}

/// One line for the diagnostic and one for each label, each naming its byte
/// offset, then its help and note.
fn write_compact(label: &str, diag: &Diagnostic, output: &mut Vec<u8>) {
    let place = |span: Span| format!("{label}: byte {}", span.start);
    let at = diag.span.map_or_else(|| label.to_owned(), place);
    let (code, severity, message) = (diag.code, diag.severity.name(), &diag.message);
    let mut lines = vec![format!("[{code}] {severity}: {message} ({at})")];
    lines.extend(
        diag.labels.iter().map(|other| format!("  {} ({})", other.message, place(other.span))),
    );
    for (heading, extra) in [("help", &diag.help), ("note", &diag.note)] {
        lines.extend(extra.as_ref().map(|text| format!("{heading}: {text}")));
    }
    for line in lines {
        output.extend(line.as_bytes());
        output.push(b'\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(label: &str, text: Option<&[u8]>, diags: &[Diagnostic]) -> String {
        let mut output = Vec::new();
        let file = SourceFile { label, text, version: LuaVersion::Lua54 };
        DefaultReporter.render(file, diags, &mut output);
        String::from_utf8(output).expect("the report is UTF-8")
    }

    #[test]
    fn renders_a_diag_with_its_code_message_and_labels() {
        let diag = Diagnostic::e008(
            Span::new(19, 19),
            "expected `end` to close `function`, found end of file",
        )
        .with_label(Span::new(6, 14), "unclosed `function`");
        let text = rendered("test.lua", Some(b"local function f()"), &[diag]);
        assert!(text.contains("E008"), "{text}");
        assert!(text.contains("expected `end` to close `function`"), "{text}");
        assert!(text.contains("unclosed `function`"), "{text}");
    }

    #[test]
    fn long_line_uses_compact_rendering() {
        // A single 2 MB line would hang ariadne; the compact path must be
        // taken and finish instantly.
        let huge = format!("local x = {}", "1+".repeat(1_000_000));
        let offset = huge.len() - 1;
        let diag = Diagnostic::e008(Span::from_indices(offset, offset + 1), "boom")
            .with_label(Span::new(0, 5), "here");
        let text = rendered("big.lua", Some(huge.as_bytes()), &[diag]);
        assert_eq!(
            text,
            format!(
                "\
[E008] error: boom (big.lua: byte {offset})
  here (big.lua: byte 0)
"
            ),
            "{text}"
        );
    }

    #[test]
    fn unreadable_file_uses_compact_rendering_with_its_help() {
        let diag = Diagnostic::e010("gone.lua", "no such file");
        let text = rendered("gone.lua", None, &[diag]);
        assert_eq!(
            text,
            "\
[E010] error: cannot read file `gone.lua`: no such file (gone.lua)
help: check that the file exists and is readable
",
            "{text}"
        );
    }

    #[test]
    fn only_lua_line_breaks_still_end_a_line_at_the_same_offsets() {
        let render = |text: &str, version| renderable(text.as_bytes(), version);
        assert_eq!(
            render("a\x0Bb\x0Cc\u{85}d\u{2028}e\u{2029}f", LuaVersion::Lua54),
            "a b c  d   e   f"
        );
        assert_eq!(render("a\n\rb\r\nc\rd", LuaVersion::Lua54), "a\r\nb\r\nc\rd");
        assert_eq!(render("a\n\rb\r\nc\rd", LuaVersion::Luau), "a\n b\r\nc d");
        assert_eq!(render("\u{e9}\u{2027}\u{c5}", LuaVersion::Lua54), "\u{e9}\u{2027}\u{c5}");
    }

    #[test]
    fn a_byte_that_is_not_utf8_renders_as_one_mark_at_its_offset() {
        assert_eq!(
            renderable(b"-- caf\xE9\x0B\xC2\n\xFF\xE2\x80", LuaVersion::Lua51),
            "-- caf? ?\n???"
        );
        let diag = Diagnostic::e008(Span::new(17, 22), "boom");
        let text = rendered("latin1.lua", Some(b"local s = 'caf\xE9' local"), &[diag]);
        assert!(text.contains("latin1.lua:1:18"), "{text}");
    }

    #[test]
    fn a_short_line_renders_its_snippet() {
        let diag = Diagnostic::e008(Span::new(6, 7), "boom");
        let text = rendered("small.lua", Some(b"local x = 1\n"), &[diag]);
        assert!(text.contains("small.lua:1:7"), "{text}");
    }
}
