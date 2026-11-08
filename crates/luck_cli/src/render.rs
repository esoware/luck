//! Terminal rendering for [`Diagnostic`]s via ariadne. Library crates only
//! produce diagnostics, and the CLI prints them.

use core::fmt;
use core::ops::Range;
use std::io::{self, Write};

use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};
use luck_core::{Diagnostic, DiagnosticSeverity, native_path, read_source_file};
use luck_token::offset_to_index;
use rustc_hash::FxHashMap;

/// Above this line length (bytes), ariadne's caret rendering effectively hangs.
///
/// A minified or obfuscated file is one multi-megabyte line, so such
/// diagnostics get a compact, snippet-free rendering instead.
const MAX_RENDERABLE_LINE_LENGTH: usize = 10_000;

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

/// A path that isn't in the [`FileCache`].
#[derive(Debug)]
struct CacheMiss(String);

impl fmt::Display for CacheMiss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "file not in cache: `{}`", self.0)
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

/// The lowercase name of a severity, as JSON output and compact rendering
/// spell it.
pub(crate) const fn severity_label(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
    }
}

/// Prints diagnostics to stderr, reading the files they point into from disk.
pub(crate) fn print_diags(diags: &[Diagnostic]) {
    let mut cache = FileCache::from_disk(diags);
    write_diags(diags, &mut cache, &mut io::stderr());
}

/// Prints diagnostics that all point into `source`, a document that isn't on
/// disk, such as stdin or a bundle being minified, to stderr.
pub(crate) fn print_document_diags(path: String, source: String, diags: &[Diagnostic]) {
    let mut cache = FileCache::with_file(path, source);
    write_diags(diags, &mut cache, &mut io::stderr());
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

/// Writes each diagnostic with ariadne's pretty-printer, or a compact
/// fallback when its source line is pathologically long.
fn write_diags(diags: &[Diagnostic], cache: &mut FileCache, output: &mut impl Write) {
    for diag in diags {
        if is_line_too_long(diag, cache) {
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

/// Writes a one-line diagnostic with no source snippet, the fallback for a
/// line too long for ariadne.
fn write_compact(diag: &Diagnostic, output: &mut impl Write) {
    let _unused = writeln!(
        output,
        "[{}] {}: {} ({}: byte {})",
        diag.code,
        severity_label(diag.severity),
        diag.message,
        diag.file_path,
        diag.span.start
    );
}

fn is_line_too_long(diag: &Diagnostic, cache: &FileCache) -> bool {
    cache.line_length_at(&diag.file_path, offset_to_index(diag.span.start))
        > MAX_RENDERABLE_LINE_LENGTH
}

#[cfg(test)]
mod tests {
    use luck_token::Span;

    use super::*;

    #[test]
    fn file_cache_fetches_only_the_files_it_holds() {
        use ariadne::Cache as _;
        let mut cache = FileCache::with_file("test.lua".to_owned(), "local x = 1".to_owned());
        cache.fetch(&"test.lua").expect("test.lua is cached");
        cache.fetch(&"nonexistent.lua").expect_err("nonexistent.lua is not cached");
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
        let diag = Diagnostic::e008("big.lua", Span::from_indices(offset, offset + 1), "boom");
        let rendered = render_document_diags("big.lua".to_owned(), huge, &[diag]);
        let text = String::from_utf8(rendered).unwrap();
        assert_eq!(
            text,
            format!("[E008] error: parse error: boom (big.lua: byte {offset})\n"),
            "{text}"
        );
    }

    #[test]
    fn short_line_is_not_too_long() {
        let cache = FileCache::with_file("small.lua".to_owned(), "local x = 1\n".to_owned());
        let diag = Diagnostic::e008("small.lua", Span::new(6, 7), "boom");
        assert!(!is_line_too_long(&diag, &cache));
    }
}
