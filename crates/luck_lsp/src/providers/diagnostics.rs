use luck_linter::{LintDiagnostic, Severity};
use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

use crate::document::DocumentState;

/// The source string in `Diagnostic.source`; VS Code shows it in tooltips.
const DIAGNOSTIC_SOURCE: &str = "luck";

pub(crate) fn to_lsp(document: &DocumentState, lints: &[LintDiagnostic]) -> Vec<Diagnostic> {
    lints
        .iter()
        .map(|lint| Diagnostic {
            range: document.range(lint.span),
            severity: Some(match lint.severity {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
            }),
            code: Some(NumberOrString::String(lint.rule.to_owned())),
            code_description: None,
            source: Some(DIAGNOSTIC_SOURCE.to_owned()),
            message: lint.help.as_ref().map_or_else(
                || lint.message.clone(),
                |help| format!("{}\n\nhelp: {help}", lint.message),
            ),
            related_information: None,
            tags: None,
            data: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use luck_linter::Category;
    use luck_token::Span;
    use rustc_hash::FxHashSet;

    use super::*;

    fn convert(lint: LintDiagnostic) -> Diagnostic {
        let document =
            DocumentState::new("local x = 1".to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
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
            message: "bad".to_owned(),
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
            message: "stylistic issue".to_owned(),
            span: Span::new(0, 5),
            help: Some("rename it".to_owned()),
            fix: None,
        });
        assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diag.message, "stylistic issue\n\nhelp: rename it");
    }
}
