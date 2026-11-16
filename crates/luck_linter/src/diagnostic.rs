use core::iter;

use luck_token::{LuaVersion, Span, chunk_start, index_to_offset, line_breaks, offset_to_index};

use crate::fix::Fix;

pub type Severity = luck_core::DiagnosticSeverity;

/// A group of lint rules, which `categories` sets the level of together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Outright wrong or useless code.
    Correctness,
    /// Most likely wrong or useless.
    Suspicious,
    /// Idiomatic code style.
    Style,
    /// Performance improvements.
    Performance,
}

impl Category {
    /// The name `luck.json`, `--rules`, and `--print-config` spell it with.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Correctness => "correctness",
            Self::Suspicious => "suspicious",
            Self::Style => "style",
            Self::Performance => "performance",
        }
    }

    /// Parses a `-A`/`-W`/`-D` flag value, case-insensitively, or `None` when
    /// it names no category.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "correctness" => Some(Self::Correctness),
            "suspicious" => Some(Self::Suspicious),
            "style" => Some(Self::Style),
            "performance" | "perf" => Some(Self::Performance),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintDiagnostic {
    pub rule: &'static str,
    pub category: Category,
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    pub help: Option<String>,
    pub fix: Option<Fix>,
}

/// The driver makes each one a [`LintDiagnostic`] carrying the reporting
/// rule's name and category and its resolved severity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub message: String,
    pub span: Span,
    pub help: Option<String>,
    pub fix: Option<Fix>,
}

impl Finding {
    #[must_use]
    pub(crate) fn new<Message>(message: Message, span: Span) -> Self
    where
        Message: Into<String>,
    {
        Self { message: message.into(), span, help: None, fix: None }
    }

    #[must_use]
    pub(crate) fn with_help<Help>(mut self, help: Help) -> Self
    where
        Help: Into<String>,
    {
        self.help = Some(help.into());
        self
    }

    #[must_use]
    pub(crate) fn with_fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }

    #[must_use]
    pub(crate) fn with_optional_fix(mut self, fix: Option<Fix>) -> Self {
        self.fix = fix;
        self
    }

    pub(crate) fn into_diagnostic(
        self,
        rule: &'static str,
        category: Category,
        severity: Severity,
    ) -> LintDiagnostic {
        let Self { message, span, help, fix } = self;
        LintDiagnostic { rule, category, severity, message, span, help, fix }
    }
}

/// Where each line of a source starts, for the positions diagnostic text
/// quotes.
#[derive(Debug)]
pub(crate) struct LineIndex {
    /// The first line starts past a BOM the version skips.
    line_starts: Vec<u32>,
    source_length: u32,
}

impl LineIndex {
    pub(crate) fn new(source: &str, version: LuaVersion) -> Self {
        let first_line_start = index_to_offset(chunk_start(source, version));
        let line_starts = iter::once(first_line_start)
            .chain(line_breaks(source, version).map(|line_break| index_to_offset(line_break.end)))
            .collect();
        Self { line_starts, source_length: index_to_offset(source.len()) }
    }

    /// The one-based line `offset` is on.
    pub(crate) fn line(&self, offset: u32) -> u32 {
        index_to_offset(self.line_starts.partition_point(|&start| start <= offset)).max(1)
    }

    /// One-based `(line, column)` for a byte `offset`.
    ///
    /// Columns count bytes from the line start, matching ariadne's
    /// `IndexType::Byte` rendering so a position quoted in diagnostic text
    /// lines up with what the CLI prints. A BOM that the version skips is not
    /// a column. An out-of-range offset clamps to the end of the source.
    pub(crate) fn line_column(&self, offset: u32) -> (u32, u32) {
        let clamped = offset.min(self.source_length);
        let line = self.line(clamped);
        let line_start = self
            .line_starts
            .get(offset_to_index(line.saturating_sub(1)))
            .copied()
            .unwrap_or_default();
        (line, clamped.saturating_sub(line_start).saturating_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_column_is_one_based_byte_columns() {
        let lines = LineIndex::new("local x\ndo local y\nend", LuaVersion::Lua54);
        assert_eq!(lines.line_column(0), (1, 1));
        assert_eq!(lines.line_column(6), (1, 7));
        // First byte of line 2 (the `d` in `do`).
        assert_eq!(lines.line_column(8), (2, 1));
        // `y` on line 2.
        assert_eq!(lines.line_column(17), (2, 10));
    }

    #[test]
    fn line_column_clamps_out_of_range() {
        let lines = LineIndex::new("abc\ndef", LuaVersion::Lua54);
        assert_eq!(lines.line_column(999), (2, 4));
    }

    #[test]
    fn line_column_skips_a_bom() {
        let lines = LineIndex::new("\u{feff}local x\ny", LuaVersion::Lua54);
        assert_eq!(lines.line_column(3), (1, 1));
        assert_eq!(lines.line_column(11), (2, 1));
    }
}
