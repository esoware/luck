use luck_token::Span;

/// An error or warning with a code, source span, and optional help.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// An `E`/`W` code from a numbered constructor such as
    /// [`Diagnostic::e008`], or a lint rule's name.
    pub code: &'static str,
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub file_path: String,
    pub span: Span,
    pub help: Option<String>,
}

/// Diagnostic severity: an `error` fails the run, a `warning` doesn't by
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

impl Diagnostic {
    #[must_use]
    pub const fn error_at(
        code: &'static str,
        message: String,
        file_path: String,
        span: Span,
    ) -> Self {
        Self { code, message, severity: DiagnosticSeverity::Error, file_path, span, help: None }
    }

    #[must_use]
    pub const fn warning_at(
        code: &'static str,
        message: String,
        file_path: String,
        span: Span,
    ) -> Self {
        Self { code, message, severity: DiagnosticSeverity::Warning, file_path, span, help: None }
    }

    #[must_use]
    pub(crate) fn with_help(mut self, help: String) -> Self {
        self.help = Some(help);
        self
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == DiagnosticSeverity::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_error_follows_the_constructor_severity() {
        let error = Diagnostic::error_at(
            "E001",
            "test message".to_owned(),
            "test.lua".to_owned(),
            Span::new(0, 10),
        );
        assert!(error.is_error());
        assert_eq!(error.code, "E001");

        let warning = Diagnostic::warning_at(
            "W001",
            "test warning".to_owned(),
            "test.lua".to_owned(),
            Span::new(0, 5),
        );
        assert!(!warning.is_error());
        assert_eq!(warning.severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn stays_below_result_large_err_threshold() {
        // `clippy::result_large_err` fires above 128 bytes by default.
        assert!(size_of::<Diagnostic>() <= 128);
    }
}
