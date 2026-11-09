//! Terminal rendering for [`Diagnostic`]s via ariadne. Library crates only
//! produce diagnostics, and the CLI prints them.

use core::fmt;
use core::ops::Range;
use std::io::{self, Write};

use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};
use luck_core::{Diagnostic, DiagnosticSeverity, native_path, read_source_file};
use luck_token::{SourceError, offset_to_index};
use rustc_hash::FxHashMap;

/// Above this line length, ariadne's caret rendering effectively hangs.
///
/// A minified or obfuscated file is one multi-megabyte line, so such
/// diagnostics get a compact, snippet-free rendering instead.
const MAX_RENDERABLE_LINE_BYTES: usize = 10_000;

/// Source text by file path, for ariadne to render snippets from.
#[derive(Default)]
struct FileCache {
    sources: FxHashMap<String, Source<String>>,
}

impl FileCache {
    fn with_file(path: String, source: String) -> Self {
        let mut cache = Self::default();
        cache.sources.insert(path, Source::from(source));
        cache
    }

    /// A cache holding the file each diagnostic points into, read from disk.
    /// A file that can't be read is left out.
    fn from_disk(diags: &[Diagnostic]) -> Self {
        let mut cache = Self::default();
        for diag in diags {
            if cache.sources.contains_key(&diag.file_path) {
                continue;
            }
            if let Ok(source) = read_source_file(native_path(&diag.file_path)) {
                cache.sources.insert(diag.file_path.clone(), Source::from(source));
            }
        }
        cache
    }

    /// Length in bytes of the line containing `offset`, or 0 if the file
    /// isn't cached.
    fn line_length_at(&self, path: &str, offset: usize) -> usize {
        let Some(source) = self.sources.get(path).map(Source::text) else {
            return 0;
        };
        let (before, after) = source.split_at(offset.min(source.len()));
        let line_prefix = before.rsplit('\n').next().map_or(0, str::len);
        let line_suffix = after.split('\n').next().map_or(0, str::len);
        line_prefix.saturating_add(line_suffix)
    }
}

/// A path that isn't in the [`FileCache`]. ariadne would print it to stderr
/// with `Debug`, so `write_diags` never hands it a diagnostic in such a file.
struct CacheMiss(String);

impl fmt::Debug for CacheMiss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CacheMiss").field(&self.0).finish()
    }
}

impl ariadne::Cache<&str> for FileCache {
    type Storage = String;

    fn fetch(&mut self, id: &&str) -> Result<&Source<String>, impl fmt::Debug> {
        self.sources.get(*id).ok_or_else(|| CacheMiss(id.to_string()))
    }

    fn display<'id>(&self, id: &'id &str) -> Option<impl fmt::Display + 'id> {
        Some(*id)
    }
}

/// Prints diagnostics to stderr, reading the files they point into from disk.
pub(crate) fn print_diags(diags: &[Diagnostic]) {
    let mut cache = FileCache::from_disk(diags);
    write_diags(diags, &mut cache, &mut io::stderr());
}

/// Each of `errors` in `path`, as the E008 it reports as.
fn source_error_diags(path: &str, errors: &[SourceError]) -> Vec<Diagnostic> {
    errors.iter().map(|error| Diagnostic::e008(path, error)).collect()
}

/// Prints a document's parse errors, each as an E008, to stderr. `source` is a
/// document that isn't on disk, such as stdin or a bundle being minified.
pub(crate) fn print_source_errors(path: String, source: String, errors: &[SourceError]) {
    let diags = source_error_diags(&path, errors);
    let mut cache = FileCache::with_file(path, source);
    write_diags(&diags, &mut cache, &mut io::stderr());
}

/// Renders diagnostics that all point into `source` into a buffer, so
/// parallel per-file workers can render concurrently while the caller prints
/// in input order.
pub(crate) fn render_document_diags(path: String, source: String, diags: &[Diagnostic]) -> Vec<u8> {
    let mut cache = FileCache::with_file(path, source);
    let mut rendered_bytes = Vec::new();
    write_diags(diags, &mut cache, &mut rendered_bytes);
    rendered_bytes
}

/// Renders a document's parse or validation errors, each as an E008, the same
/// way [`render_document_diags`] renders diagnostics.
pub(crate) fn render_source_errors(
    path: String,
    source: String,
    errors: &[SourceError],
) -> Vec<u8> {
    let diags = source_error_diags(&path, errors);
    render_document_diags(path, source, &diags)
}

/// Writes each diagnostic with ariadne's pretty-printer, or a compact
/// fallback when its file couldn't be read or its source line is
/// pathologically long.
fn write_diags(diags: &[Diagnostic], cache: &mut FileCache, output: &mut impl Write) {
    for diag in diags {
        if !cache.sources.contains_key(&diag.file_path) || is_line_too_long(diag, cache) {
            write_compact(diag, output);
        } else {
            // A failed write has nowhere to be reported.
            let _unused = build_report(diag).write(&mut *cache, &mut *output);
        }
    }
}

fn build_report(diag: &Diagnostic) -> Report<'_, (&str, Range<usize>)> {
    let kind = match diag.severity {
        DiagnosticSeverity::Error => ReportKind::Error,
        DiagnosticSeverity::Warning => ReportKind::Warning,
    };

    let span = (diag.file_path.as_str(), Range::from(diag.span));

    let mut builder = Report::build(kind, span.clone())
        .with_code(diag.code)
        .with_message(&diag.message)
        .with_config(Config::default().with_index_type(IndexType::Byte))
        .with_label(Label::new(span).with_message(&diag.message));

    if let Some(help) = &diag.help {
        builder = builder.with_help(help);
    }

    builder.finish()
}

/// Writes a diagnostic with no source snippet, the fallback when ariadne
/// can't render one.
fn write_compact(diag: &Diagnostic, output: &mut impl Write) {
    let help_line = diag.help.as_ref().map_or_else(String::new, |help| format!("\nhelp: {help}"));
    let _unused = writeln!(
        output,
        "[{}] {}: {} ({}: byte {}){help_line}",
        diag.code,
        diag.severity.name(),
        diag.message,
        diag.file_path,
        diag.span.start
    );
}

fn is_line_too_long(diag: &Diagnostic, cache: &FileCache) -> bool {
    cache.line_length_at(&diag.file_path, offset_to_index(diag.span.start))
        > MAX_RENDERABLE_LINE_BYTES
}

#[cfg(test)]
mod tests {
    use luck_token::Span;

    use super::*;

    fn boom_at(span: Span) -> SourceError {
        SourceError { span, message: "boom".to_owned() }
    }

    #[test]
    fn file_cache_fetches_only_the_files_it_holds() {
        use ariadne::Cache as _;
        let mut cache = FileCache::with_file("test.lua".to_owned(), "local x = 1".to_owned());
        cache.fetch(&"test.lua").expect("test.lua is cached");
        let miss = cache.fetch(&"nonexistent.lua").expect_err("nonexistent.lua is not cached");
        assert_eq!(format!("{miss:?}"), r#"CacheMiss("nonexistent.lua")"#);
    }

    #[test]
    fn renders_a_diag_with_its_code_and_message() {
        let diag = Diagnostic::e004("test.lua", Span::new(10, 23), "foo", &["foo.lua".to_owned()]);
        let message = diag.message.clone();
        let rendered = render_document_diags(
            "test.lua".to_owned(),
            "local x = require(\"foo\")\n".to_owned(),
            &[diag],
        );
        let text = String::from_utf8(rendered).unwrap();
        assert!(text.contains("E004"), "{text}");
        assert!(text.contains(&message), "{text}");
    }

    #[test]
    fn long_line_uses_compact_rendering() {
        // A single 2 MB line would hang ariadne; the compact path must be
        // taken and finish instantly.
        let huge = format!("local x = {}", "1+".repeat(1_000_000));
        let offset = huge.len() - 1;
        let diag = Diagnostic::e008("big.lua", &boom_at(Span::from_indices(offset, offset + 1)));
        let rendered = render_document_diags("big.lua".to_owned(), huge, &[diag]);
        let text = String::from_utf8(rendered).unwrap();
        assert_eq!(
            text,
            format!("[E008] error: parse error: boom (big.lua: byte {offset})\n"),
            "{text}"
        );
    }

    #[test]
    fn uncached_file_uses_compact_rendering_with_its_help() {
        let diag = Diagnostic::e010("gone.lua", Span::default(), "no such file");
        let rendered = render_document_diags("other.lua".to_owned(), String::new(), &[diag]);
        let text = String::from_utf8(rendered).unwrap();
        assert_eq!(
            text,
            "[E010] error: cannot read file `gone.lua`: no such file (gone.lua: byte 0)\n\
             help: check that the file exists and is readable\n",
            "{text}"
        );
    }

    #[test]
    fn bug_compact_rendering_covers_a_span_that_reaches_a_long_line() {
        let source = format!("(\n{}\n", "1+".repeat(10_000));
        let diag = Diagnostic::e008("big.lua", &boom_at(Span::from_indices(0, source.len())));
        let rendered = render_document_diags("big.lua".to_owned(), source, &[diag]);
        let rendered_bytes = rendered.len();
        assert!(
            rendered_bytes < 200,
            "a span reaching a line over the limit must render compactly, got {rendered_bytes} bytes"
        );
    }

    #[test]
    fn short_line_is_not_too_long() {
        let cache = FileCache::with_file("small.lua".to_owned(), "local x = 1\n".to_owned());
        let diag = Diagnostic::e008("small.lua", &boom_at(Span::new(6, 7)));
        assert!(!is_line_too_long(&diag, &cache));
    }
}
