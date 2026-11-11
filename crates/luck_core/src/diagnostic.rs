mod codes;

use luck_token::Span;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

impl DiagnosticSeverity {
    /// The name JSON output spells it with.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

impl Diagnostic {
    fn error_at<Message>(code: &'static str, message: Message, file_path: &str, span: Span) -> Self
    where
        Message: Into<String>,
    {
        Self {
            code,
            message: message.into(),
            severity: DiagnosticSeverity::Error,
            file_path: file_path.to_owned(),
            span,
            help: None,
        }
    }

    fn warning_at<Message>(
        code: &'static str,
        message: Message,
        file_path: &str,
        span: Span,
    ) -> Self
    where
        Message: Into<String>,
    {
        Self {
            code,
            message: message.into(),
            severity: DiagnosticSeverity::Warning,
            file_path: file_path.to_owned(),
            span,
            help: None,
        }
    }

    #[must_use]
    fn with_help<Help>(mut self, help: Help) -> Self
    where
        Help: Into<String>,
    {
        self.help = Some(help.into());
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
    fn stays_below_result_large_err_threshold() {
        // `clippy::result_large_err` fires above 128 bytes by default.
        assert!(size_of::<Diagnostic>() <= 128);
    }
}
