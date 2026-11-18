use luck_linter::{LintDiagnostic, Severity};
use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range};

use crate::document::DocumentState;
use crate::line_index::LineIndex;

/// The source string in `Diagnostic.source`; VS Code shows it in tooltips.
const DIAGNOSTIC_SOURCE: &str = "luck";

/// `lints` come sorted by where they start, as the linter returns them.
pub(crate) fn to_lsp(document: &DocumentState, lints: &[LintDiagnostic]) -> Vec<Diagnostic> {
    let mut positions = document.positions();
    lints
        .iter()
        .map(|lint| {
            let message = lint.help.as_ref().map_or_else(
                || lint.message.clone().into_owned(),
                |help| format!("{}\n\nhelp: {help}", lint.message),
            );
            diagnostic(positions.range(lint.span), lint.severity, lint.rule, message)
        })
        .collect()
}

/// A problem in `text`, a config file that isn't open, such as a `luck.json`
/// that doesn't load.
pub(crate) fn config_to_lsp(diag: &luck_core::Diagnostic, text: &str) -> Diagnostic {
    let message = diag
        .help
        .as_ref()
        .map_or_else(|| diag.message.clone(), |help| format!("{}\n\nhelp: {help}", diag.message));
    let range = LineIndex::new(text).range(text, diag.span);
    diagnostic(range, diag.severity, diag.code, message)
}

fn diagnostic(range: Range, severity: Severity, code: &str, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(match severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(code.to_owned())),
        code_description: None,
        source: Some(DIAGNOSTIC_SOURCE.to_owned()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use luck_linter::Category;
    use luck_token::Span;

    use super::*;
    use crate::fixtures::document;

    fn convert(lint: LintDiagnostic) -> Diagnostic {
        let document = document("local x = 1", LuaTarget::Lua54);
        let mut diags = to_lsp(&document, &[lint]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        diags.remove(0)
    }

    #[test]
    fn maps_error_severity() {
        let diag = convert(LintDiagnostic {
            rule: "test_rule",
            category: Category::Correctness,
            severity: Severity::Error,
            message: "bad".into(),
            span: Span::new(6, 7),
            help: None,
            fix: None,
        });
        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diag.message, "bad");
        assert_eq!(diag.code, Some(NumberOrString::String("test_rule".to_owned())));
    }

    #[test]
    fn maps_warning_severity_and_appends_help() {
        let diag = convert(LintDiagnostic {
            rule: "warn_rule",
            category: Category::Style,
            severity: Severity::Warning,
            message: "stylistic issue".into(),
            span: Span::new(0, 5),
            help: Some("rename it".into()),
            fix: None,
        });
        assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            diag.message,
            "\
stylistic issue

help: rename it"
        );
    }
}
