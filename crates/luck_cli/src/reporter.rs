//! Library crates only produce diagnostics, and the CLI writes them, each
//! output format behind a [`Reporter`], as oxlint's are.

mod default;
mod github;
mod json;

use std::io::{self, Write as _};

use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_resolver::{FileSystem as _, FileSystemOs, to_slash_lossy};
use luck_token::{LineIndex, LuaVersion};

pub(crate) use default::DefaultReporter;
pub(crate) use github::GithubReporter;
pub(crate) use json::JsonReporter;

/// The file a reporter's diagnostics are in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SourceFile<'text> {
    /// What the output calls it.
    pub label: &'text str,
    /// Bytes, as Lua reads a source, or `None` for a file that can't be
    /// read, which no snippet shows.
    pub text: Option<&'text [u8]>,
    pub version: LuaVersion,
}

/// One output format.
pub(crate) trait Reporter {
    /// Appends what `diags`, all in `file`, render as to `output`, so
    /// parallel workers can render while the caller prints in input order.
    fn render(&self, file: SourceFile<'_>, diags: &[Diagnostic], output: &mut Vec<u8>);
}

/// Writes `files`' diagnostics to stderr in the default format, each file's
/// text read from disk.
pub(crate) fn print_files(files: &[FileDiagnostics], version: LuaVersion) {
    let mut output = Vec::new();
    for file in files {
        let text = FileSystemOs.read(&file.path).ok();
        let label = to_slash_lossy(&file.path);
        let source = SourceFile { label: &label, text: text.as_deref(), version };
        DefaultReporter.render(source, &file.diags, &mut output);
    }
    // A failed write has nowhere to be reported.
    let _ = io::stderr().write_all(&output);
}

/// Writes the diagnostics in `text`, a document that isn't on disk, such as
/// stdin or a bundle being minified, to stderr in the default format.
pub(crate) fn print_document(label: &str, text: &[u8], diags: &[Diagnostic], version: LuaVersion) {
    let mut output = Vec::new();
    DefaultReporter.render(SourceFile { label, text: Some(text), version }, diags, &mut output);
    let _ = io::stderr().write_all(&output);
}

/// The 1-based line and character column of the byte at `offset` in
/// `text`, for the machine-readable formats. A byte that isn't UTF-8 is a
/// character of its own, as a one-byte encoding reads it.
fn line_and_column(lines: &LineIndex, text: &[u8], offset: u32) -> (u32, usize) {
    let (line, byte_column) = lines.line_column(offset);
    let index = offset as usize;
    let line_start = index - (byte_column - 1) as usize;
    let characters_before = text.get(line_start..index).map_or(0, |before| {
        before
            .utf8_chunks()
            .map(|chunk| chunk.valid().chars().count() + chunk.invalid().len())
            .sum()
    });
    (line, characters_before + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_counts_characters_from_one() {
        let text = "\
local a
local \u{e9}b = 1
"
        .as_bytes();
        let lines = LineIndex::new(text, LuaVersion::Lua54);
        assert_eq!(line_and_column(&lines, text, 0), (1, 1));
        assert_eq!(line_and_column(&lines, text, 6), (1, 7));
        assert_eq!(line_and_column(&lines, text, 8), (2, 1));
        // `b` follows the two-byte `e` with an acute accent.
        assert_eq!(line_and_column(&lines, text, 16), (2, 8));
    }

    #[test]
    fn line_and_column_starts_the_first_line_after_a_skipped_bom() {
        let text = "\u{feff}local a".as_bytes();
        let lines = LineIndex::new(text, LuaVersion::Lua54);
        assert_eq!(line_and_column(&lines, text, 3), (1, 1));
        assert_eq!(line_and_column(&lines, text, 9), (1, 7));
    }

    #[test]
    fn line_and_column_counts_a_byte_that_is_not_utf8_as_one_character() {
        let text = b"local s = 'caf\xE9' local";
        let lines = LineIndex::new(text, LuaVersion::Lua51);
        assert_eq!(line_and_column(&lines, text, 17), (1, 18));
    }
}
