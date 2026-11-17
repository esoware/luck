//! Library crates only produce diagnostics, and the CLI prints them.

use core::fmt;
use core::ops::Range;
use std::io::{self, Write};

use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};
use luck_core::{Diagnostic, DiagnosticSeverity};
use luck_resolver::{native_path, read_source_file};
use luck_token::{LineIndex, LuaVersion, SourceError, chunk_start, offset_to_index};
use rustc_hash::FxHashMap;

/// Above this line length, ariadne's caret rendering effectively hangs.
///
/// A minified or obfuscated file is one multi-megabyte line, so such
/// diagnostics get a compact, snippet-free rendering instead.
const MAX_RENDERABLE_LINE_CHARS: usize = 10_000;

struct CachedFile {
    source: Source<String>,
    /// How far spans shift to reach `source`, which drops a BOM the file's
    /// dialect skips.
    span_shift: usize,
}

impl CachedFile {
    fn new(text: &str, version: LuaVersion) -> Self {
        let span_shift = chunk_start(text, version);
        let body = text.get(span_shift..).unwrap_or_default();
        Self { source: Source::from(with_only_lua_line_breaks(body, version)), span_shift }
    }
}

/// `text` with only `version`'s line breaks still ending a line for ariadne,
/// whose own set adds VT, FF, NEL, LS, and PS and counts LFCR as two. Every
/// byte offset stays the same.
fn with_only_lua_line_breaks(text: &str, version: LuaVersion) -> String {
    let bytes = text.as_bytes();
    let mut rendered = String::with_capacity(text.len());
    let mut copied_to = 0;
    let mut index = 0;
    while let Some(&b) = bytes.get(index) {
        let next = |ahead: usize| bytes.get(index.saturating_add(ahead)).copied();
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
                index = index.saturating_add(1);
                continue;
            }
        };
        rendered.push_str(text.get(copied_to..index).unwrap_or_default());
        rendered.push_str(replacement);
        index = index.saturating_add(replacement.len());
        copied_to = index;
    }
    rendered.push_str(text.get(copied_to..).unwrap_or_default());
    rendered
}

#[derive(Default)]
struct FileCache {
    files: FxHashMap<String, CachedFile>,
}

impl FileCache {
    fn with_file(path: String, source: &str, version: LuaVersion) -> Self {
        let mut cache = Self::default();
        cache.files.insert(path, CachedFile::new(source, version));
        cache
    }

    /// A file that can't be read is left out.
    fn from_disk(diags: &[Diagnostic], version: LuaVersion) -> Self {
        let mut cache = Self::default();
        for diag in diags {
            if cache.files.contains_key(&diag.file_path) {
                continue;
            }
            if let Ok(source) = read_source_file(native_path(&diag.file_path)) {
                cache.files.insert(diag.file_path.clone(), CachedFile::new(&source, version));
            }
        }
        cache
    }

    /// Length in characters of the line containing `offset`, or 0 if the file
    /// isn't cached.
    fn line_length_at(&self, path: &str, offset: usize) -> usize {
        let Some(file) = self.files.get(path) else {
            return 0;
        };
        let body_offset = offset.saturating_sub(file.span_shift);
        file.source.get_byte_line(body_offset).map_or(0, |(line, _, _)| line.len())
    }

    fn span_shift(&self, path: &str) -> usize {
        self.files.get(path).map_or(0, |file| file.span_shift)
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
        self.files.get(*id).map(|file| &file.source).ok_or_else(|| CacheMiss(id.to_string()))
    }

    fn display<'id>(&self, id: &'id &str) -> Option<impl fmt::Display + 'id> {
        Some(*id)
    }
}

pub(crate) fn print_diags(diags: &[Diagnostic], version: LuaVersion) {
    let mut cache = FileCache::from_disk(diags, version);
    write_diags(diags, &mut cache, &mut io::stderr());
}

fn source_error_diags(path: &str, errors: &[SourceError]) -> Vec<Diagnostic> {
    errors.iter().map(|e| Diagnostic::e008(path, e)).collect()
}

/// `source` is a document that isn't on disk, such as stdin or a bundle being
/// minified.
pub(crate) fn print_source_errors(
    path: String,
    source: &str,
    errors: &[SourceError],
    version: LuaVersion,
) {
    let diags = source_error_diags(&path, errors);
    let mut cache = FileCache::with_file(path, source, version);
    write_diags(&diags, &mut cache, &mut io::stderr());
}

/// Renders diagnostics that all point into `source` into a buffer, so
/// parallel per-file workers can render concurrently while the caller prints
/// in input order.
pub(crate) fn render_document_diags(
    path: String,
    source: &str,
    diags: &[Diagnostic],
    version: LuaVersion,
) -> Vec<u8> {
    let mut cache = FileCache::with_file(path, source, version);
    let mut rendered_bytes = Vec::new();
    write_diags(diags, &mut cache, &mut rendered_bytes);
    rendered_bytes
}

pub(crate) fn render_source_errors(
    path: String,
    source: &str,
    errors: &[SourceError],
    version: LuaVersion,
) -> Vec<u8> {
    let diags = source_error_diags(&path, errors);
    render_document_diags(path, source, &diags, version)
}

/// The 1-based line and character column of the byte at `offset` in
/// `source`, for the machine-readable formats.
pub(crate) fn line_and_column(lines: &LineIndex, source: &str, offset: u32) -> (u32, usize) {
    let (line, byte_column) = lines.line_column(offset);
    let index = offset_to_index(offset);
    let line_start = index.saturating_sub(offset_to_index(byte_column.saturating_sub(1)));
    let characters_before = source.get(line_start..index).map_or(0, |text| text.chars().count());
    (line, characters_before.saturating_add(1))
}

fn write_diags(diags: &[Diagnostic], cache: &mut FileCache, output: &mut impl Write) {
    for diag in diags {
        if !cache.files.contains_key(&diag.file_path) || is_line_too_long(diag, cache) {
            write_compact(diag, output);
        } else {
            let span_shift = cache.span_shift(&diag.file_path);
            // A failed write has nowhere to be reported.
            let _unused = build_report(diag, span_shift).write(&mut *cache, &mut *output);
        }
    }
}

fn build_report(diag: &Diagnostic, span_shift: usize) -> Report<'_, (&str, Range<usize>)> {
    let kind = match diag.severity {
        DiagnosticSeverity::Error => ReportKind::Error,
        DiagnosticSeverity::Warning => ReportKind::Warning,
    };

    let Range { start, end } = Range::<usize>::from(diag.span);
    let span =
        (diag.file_path.as_str(), start.saturating_sub(span_shift)..end.saturating_sub(span_shift));

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
        > MAX_RENDERABLE_LINE_CHARS
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
        let mut cache =
            FileCache::with_file("test.lua".to_owned(), "local x = 1", LuaVersion::Lua54);
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
            "local x = require(\"foo\")\n",
            &[diag],
            LuaVersion::Lua54,
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
        let rendered =
            render_document_diags("big.lua".to_owned(), &huge, &[diag], LuaVersion::Lua54);
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
        let rendered =
            render_document_diags("other.lua".to_owned(), "", &[diag], LuaVersion::Lua54);
        let text = String::from_utf8(rendered).unwrap();
        assert_eq!(
            text,
            "\
[E010] error: cannot read file `gone.lua`: no such file (gone.lua: byte 0)
help: check that the file exists and is readable
",
            "{text}"
        );
    }

    #[test]
    fn line_and_column_counts_characters_from_one() {
        let source = "\
local a
local \u{e9}b = 1
";
        let lines = LineIndex::new(source, LuaVersion::Lua54);
        assert_eq!(line_and_column(&lines, source, 0), (1, 1));
        assert_eq!(line_and_column(&lines, source, 6), (1, 7));
        assert_eq!(line_and_column(&lines, source, 8), (2, 1));
        // `b` follows the two-byte `e` with an acute accent.
        assert_eq!(line_and_column(&lines, source, 16), (2, 8));
    }

    #[test]
    fn line_and_column_starts_the_first_line_after_a_skipped_bom() {
        let source = "\u{feff}local a";
        let lines = LineIndex::new(source, LuaVersion::Lua54);
        assert_eq!(line_and_column(&lines, source, 3), (1, 1));
        assert_eq!(line_and_column(&lines, source, 9), (1, 7));
    }

    #[test]
    fn only_lua_line_breaks_still_end_a_line_at_the_same_offsets() {
        assert_eq!(
            with_only_lua_line_breaks("a\x0Bb\x0Cc\u{85}d\u{2028}e\u{2029}f", LuaVersion::Lua54),
            "a b c  d   e   f"
        );
        assert_eq!(
            with_only_lua_line_breaks("a\n\rb\r\nc\rd", LuaVersion::Lua54),
            "a\r\nb\r\nc\rd"
        );
        assert_eq!(with_only_lua_line_breaks("a\n\rb\r\nc\rd", LuaVersion::Luau), "a\n b\r\nc d");
        assert_eq!(
            with_only_lua_line_breaks("\u{e9}\u{2027}\u{c5}", LuaVersion::Lua54),
            "\u{e9}\u{2027}\u{c5}"
        );
    }

    #[test]
    fn short_line_is_not_too_long() {
        let cache =
            FileCache::with_file("small.lua".to_owned(), "local x = 1\n", LuaVersion::Lua54);
        let diag = Diagnostic::e008("small.lua", &boom_at(Span::new(6, 7)));
        assert!(!is_line_too_long(&diag, &cache));
    }
}
