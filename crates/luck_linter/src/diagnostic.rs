use luck_token::Span;

/// Severity of a lint diagnostic. Unified with the toolchain-wide
/// severity in `luck_core`; the `Error`/`Warning` variant names are
/// identical, so existing `Severity::Warning` uses keep compiling.
pub type Severity = luck_core::DiagnosticSeverity;

/// Category of a lint rule. Owned by `luck_core` so the whole config
/// deserializes there; re-exported here for the rule code.
pub use luck_core::Category;

#[derive(Debug, Clone, PartialEq)]
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
    /// Build a diagnostic for `rule` at `span`. `category` and `severity` are
    /// placeholders: the lint driver overwrites them with the rule's
    /// `category()` and the resolved (override-or-default) severity, so a rule
    /// never restates its own category/severity per diagnostic.
    #[must_use]
    pub fn new(rule: &'static str, message: impl Into<String>, span: Span) -> Self {
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
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    #[must_use]
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }

    /// Attach optional help text (for rules that compute `Option<String>`).
    #[must_use]
    pub fn with_help_opt(mut self, help: Option<String>) -> Self {
        self.help = help;
        self
    }

    /// Attach an optional auto-fix (for rules that compute `Option<Fix>`).
    #[must_use]
    pub fn with_fix_opt(mut self, fix: Option<Fix>) -> Self {
        self.fix = fix;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    pub description: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextEdit {
    pub span: Span,
    pub replacement: String,
}
