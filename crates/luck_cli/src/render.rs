//! Terminal rendering for [`Diagnostic`]s via ariadne. Lives in the CLI -
//! library crates produce diagnostics, only the CLI prints them.

use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};
use luck_core::diagnostics::{Diagnostic, DiagnosticSeverity};
use std::collections::HashMap;
use std::fmt;

/// Above this line length (bytes), ariadne's caret rendering degrades from
/// slow to effectively hung - a minified/obfuscated file is a single
/// multi-megabyte line. Such diagnostics get a compact, snippet-free
/// rendering instead.
const MAX_RENDERABLE_LINE: usize = 10_000;

/// Holds source text keyed by file path, used by ariadne to render diagnostics.
pub(crate) struct FileCache {
    sources: HashMap<String, Source<String>>,
    /// Raw text kept alongside the ariadne `Source` so we can measure line
    /// lengths without walking ariadne's line index.
    raw: HashMap<String, String>,
}

impl FileCache {
    pub(crate) fn new() -> Self {
        Self {
            sources: HashMap::new(),
            raw: HashMap::new(),
        }
    }

    pub(crate) fn add_file(&mut self, path: String, source: String) {
        self.raw.insert(path.clone(), source.clone());
        self.sources.insert(path, Source::from(source));
    }

    /// Length in bytes of the line containing `offset`, or 0 if the file
    /// is not cached. Used to decide whether ariadne can render it.
    fn line_len_at(&self, path: &str, offset: usize) -> usize {
        let Some(source) = self.raw.get(path) else {
            return 0;
        };
        let offset = offset.min(source.len());
        let start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
        let end = source[offset..]
            .find('\n')
            .map_or(source.len(), |index| offset + index);
        end - start
    }
}

impl Default for FileCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned when a file path is not found in the [`FileCache`].
#[derive(Debug)]
struct CacheMiss(String);

impl fmt::Display for CacheMiss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "File not in cache: {}", self.0)
    }
}

impl ariadne::Cache<&str> for FileCache {
    type Storage = String;

    fn fetch(&mut self, id: &&str) -> Result<&Source<String>, impl fmt::Debug> {
        self.sources
            .get(*id)
            .ok_or_else(|| CacheMiss(id.to_string()))
    }

    fn display<'a>(&self, id: &'a &str) -> Option<impl fmt::Display + 'a> {
        Some(*id)
    }
}

fn build_report<'a>(diag: &'a Diagnostic) -> Report<'a, (&'a str, std::ops::Range<usize>)> {
    let kind = match diag.severity {
        DiagnosticSeverity::Error => ReportKind::Error,
        DiagnosticSeverity::Warning => ReportKind::Warning,
    };

    let span = (diag.file_path.as_str(), diag.span.clone());

    let mut builder = Report::build(kind, span)
        .with_code(&diag.code)
        .with_message(&diag.message)
        .with_config(Config::default().with_index_type(IndexType::Byte));

    for (label_span, label_msg) in diag.labels() {
        builder = builder.with_label(
            Label::new((diag.file_path.as_str(), label_span.clone())).with_message(label_msg),
        );
    }

    if diag.labels().is_empty() {
        builder = builder.with_label(
            Label::new((diag.file_path.as_str(), diag.span.clone())).with_message(&diag.message),
        );
    }

    if let Some(help) = diag.help() {
        builder = builder.with_help(help);
    }

    builder.finish()
}

/// A compact one-line diagnostic with no source snippet - the fallback for
/// files whose relevant line is too long for ariadne to render.
fn write_compact(diag: &Diagnostic, out: &mut impl std::io::Write) {
    let severity = match diag.severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
    };
    let _ = writeln!(
        out,
        "[{}] {severity}: {} ({}: byte {})",
        diag.code, diag.message, diag.file_path, diag.span.start
    );
}

/// Whether this diagnostic sits on a line too long for ariadne to render.
fn is_line_too_long(diag: &Diagnostic, cache: &FileCache) -> bool {
    cache.line_len_at(&diag.file_path, diag.span.start) > MAX_RENDERABLE_LINE
}

/// Prints a single diagnostic to stderr using ariadne's pretty-printer,
/// or a compact fallback when the source line is pathologically long.
pub(crate) fn render_diagnostic(diag: &Diagnostic, cache: &mut FileCache) {
    if is_line_too_long(diag, cache) {
        write_compact(diag, &mut std::io::stderr());
        return;
    }
    // Ignore broken pipe
    let _ = build_report(diag).eprint(cache);
}

/// Prints all diagnostics to stderr.
pub(crate) fn render_diagnostics(diagnostics: &[Diagnostic], cache: &mut FileCache) {
    for diag in diagnostics {
        render_diagnostic(diag, cache);
    }
}

/// Renders all diagnostics into a buffer. Parallel per-file workers
/// render into buffers concurrently; the caller prints them in input
/// order so output stays deterministic.
pub(crate) fn render_diagnostics_to_buffer(
    diagnostics: &[Diagnostic],
    cache: &mut FileCache,
) -> Vec<u8> {
    let mut out = Vec::new();
    for diag in diagnostics {
        if is_line_too_long(diag, cache) {
            write_compact(diag, &mut out);
        } else {
            let _ = build_report(diag).write(&mut *cache, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_core::diagnostics::errors;

    #[test]
    fn test_file_cache() {
        use ariadne::Cache;
        let mut cache = FileCache::new();
        cache.add_file("test.lua".to_string(), "local x = 1".to_string());
        assert!(cache.fetch(&"test.lua").is_ok());
        assert!(cache.fetch(&"nonexistent.lua").is_err());
    }

    #[test]
    fn test_render_diagnostic_does_not_panic() {
        let mut cache = FileCache::new();
        cache.add_file(
            "test.lua".to_string(),
            "local x = require(\"foo\")\n".to_string(),
        );

        let diag = errors::e004("test.lua", 10..23, "foo", &["foo.lua".to_string()]);
        render_diagnostic(&diag, &mut cache);
    }

    #[test]
    fn long_line_uses_compact_rendering() {
        // A single 2 MB line would hang ariadne; the compact path must be
        // taken and finish instantly.
        let mut cache = FileCache::new();
        let huge = format!("local x = {}", "1+".repeat(1_000_000));
        let offset = huge.len() - 1;
        cache.add_file("big.lua".to_string(), huge);
        assert!(cache.line_len_at("big.lua", offset) > MAX_RENDERABLE_LINE);

        let diag = errors::e008("big.lua", offset..offset + 1, "boom");
        let mut out = Vec::new();
        write_compact(&diag, &mut out);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("E008"));
        assert!(text.contains("byte"));
    }

    #[test]
    fn short_line_not_flagged() {
        let mut cache = FileCache::new();
        cache.add_file("small.lua".to_string(), "local x = 1\n".to_string());
        let diag = errors::e008("small.lua", 6..7, "boom");
        assert!(!is_line_too_long(&diag, &cache));
    }
}
