//! Convert `luck_linter::LintDiagnostic` values into LSP `Diagnostic`s.
//!
//! Positions come from `LineIndex` so that surrogate-pair handling stays
//! consistent across formatting and lint diagnostics.

use luck_linter::diagnostic::{LintDiagnostic, Severity};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

use crate::line_index::LineIndex;

/// The source string in `Diagnostic.source` - VS Code surfaces this in tooltips.
pub const DIAGNOSTIC_SOURCE: &str = "luck";

#[must_use]
pub fn to_lsp_diagnostics(
    source: &str,
    line_index: &LineIndex,
    diags: &[LintDiagnostic],
) -> Vec<Diagnostic> {
    diags
        .iter()
        .map(|diag| to_lsp_diagnostic(source, line_index, diag))
        .collect()
}

fn to_lsp_diagnostic(source: &str, line_index: &LineIndex, diag: &LintDiagnostic) -> Diagnostic {
    let range = line_index.range(source, diag.span.start, diag.span.end);
    let message = match &diag.help {
        Some(help) => format!("{}\n\nhelp: {help}", diag.message),
        None => diag.message.clone(),
    };
    Diagnostic {
        range,
        severity: Some(map_severity(diag.severity)),
        code: Some(NumberOrString::String(diag.rule.to_string())),
        code_description: None,
        source: Some(DIAGNOSTIC_SOURCE.to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

fn map_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_linter::diagnostic::{Category, LintDiagnostic, Severity};
    use luck_token::Span;

    #[test]
    fn maps_error_severity() {
        let source = "local x = 1";
        let line_index = LineIndex::new(source);
        let diag = LintDiagnostic {
            rule: "test_rule",
            category: Category::Correctness,
            severity: Severity::Error,
            message: "bad".to_string(),
            span: Span::new(6, 7),
            help: None,
            fix: None,
        };
        let lsp = to_lsp_diagnostic(source, &line_index, &diag);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(lsp.message, "bad");
        assert_eq!(
            lsp.code,
            Some(NumberOrString::String("test_rule".to_string()))
        );
    }

    #[test]
    fn maps_warning_severity_and_appends_help() {
        let source = "local x = 1";
        let line_index = LineIndex::new(source);
        let diag = LintDiagnostic {
            rule: "warn_rule",
            category: Category::Style,
            severity: Severity::Warning,
            message: "stylistic issue".to_string(),
            span: Span::new(0, 5),
            help: Some("rename it".to_string()),
            fix: None,
        };
        let lsp = to_lsp_diagnostic(source, &line_index, &diag);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::WARNING));
        assert!(lsp.message.contains("stylistic issue"));
        assert!(lsp.message.contains("rename it"));
    }
}
