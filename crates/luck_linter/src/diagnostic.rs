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

/// One-based `(line, column)` for a byte `offset` into `source`.
///
/// Columns count bytes from the line start, matching ariadne's
/// `IndexType::Byte` rendering so a position quoted in diagnostic text lines
/// up with what the CLI prints. A BOM that `version` skips is not a column.
/// An out-of-range offset clamps to the end of the source.
#[must_use]
pub(crate) fn line_column(source: &str, offset: u32, version: LuaVersion) -> (u32, u32) {
    let end = offset_to_index(offset).min(source.len());
    let prefix = source.get(..end).unwrap_or_default();
    let mut line_count = 0usize;
    let mut line_start = chunk_start(source, version);
    for line_break in line_breaks(prefix, version) {
        line_count = line_count.saturating_add(1);
        line_start = line_break.end;
    }
    let column = end.saturating_sub(line_start);
    (index_to_offset(line_count).saturating_add(1), index_to_offset(column).saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_column_is_one_based_byte_columns() {
        let source = "local x\ndo local y\nend";
        assert_eq!(line_column(source, 0, LuaVersion::Lua54), (1, 1));
        assert_eq!(line_column(source, 6, LuaVersion::Lua54), (1, 7));
        // First byte of line 2 (the `d` in `do`).
        assert_eq!(line_column(source, 8, LuaVersion::Lua54), (2, 1));
        // `y` on line 2.
        assert_eq!(line_column(source, 17, LuaVersion::Lua54), (2, 10));
    }

    #[test]
    fn line_column_clamps_out_of_range() {
        let source = "abc\ndef";
        assert_eq!(line_column(source, 999, LuaVersion::Lua54), (2, 4));
    }
}
