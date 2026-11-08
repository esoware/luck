use luck_token::{Span, index_to_offset, offset_to_index};

use crate::fix::Fix;

/// Severity of a lint diagnostic, shared with the toolchain-wide
/// severity in `luck_core`.
pub type Severity = luck_core::DiagnosticSeverity;

/// Category of a lint rule. Defined in `luck_core` so the config can
/// deserialize it.
pub use luck_core::Category;

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

impl LintDiagnostic {
    /// Builds a diagnostic for `rule` at `span`.
    ///
    /// `category` and `severity` are placeholders. The driver overwrites them
    /// with the rule's `category()` and its configured or default severity.
    #[must_use]
    pub(crate) fn new<Message>(rule: &'static str, message: Message, span: Span) -> Self
    where
        Message: Into<String>,
    {
        Self {
            rule,
            category: Category::Correctness,
            severity: Severity::Warning,
            message: message.into(),
            span,
            help: None,
            fix: None,
        }
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
}

/// One-based `(line, column)` for a byte `offset` into `source`.
///
/// Columns count bytes from the line start, matching ariadne's
/// `IndexType::Byte` rendering so a position quoted in diagnostic text lines
/// up with what the CLI prints. An out-of-range offset clamps to the end of
/// the source.
#[must_use]
pub(crate) fn line_column(source: &str, offset: u32) -> (u32, u32) {
    let end = offset_to_index(offset).min(source.len());
    let prefix = source.as_bytes().get(..end).unwrap_or_default();
    let mut lines = prefix.rsplit(|&byte| byte == b'\n');
    let line_bytes = lines.next().map_or(0, <[u8]>::len);
    let line_breaks = lines.count();
    (index_to_offset(line_breaks).saturating_add(1), index_to_offset(line_bytes).saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_column_is_one_based_byte_columns() {
        let source = "local x\ndo local y\nend";
        assert_eq!(line_column(source, 0), (1, 1));
        assert_eq!(line_column(source, 6), (1, 7));
        // First byte of line 2 (the `d` in `do`).
        assert_eq!(line_column(source, 8), (2, 1));
        // `y` on line 2.
        assert_eq!(line_column(source, 17), (2, 10));
    }

    #[test]
    fn line_column_clamps_out_of_range() {
        let source = "abc\ndef";
        assert_eq!(line_column(source, 999), (2, 4));
    }
}
