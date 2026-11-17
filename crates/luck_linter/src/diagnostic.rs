use alloc::borrow::Cow;

use luck_token::Span;

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

/// A message or help that's a fixed text borrows it, so only one that names
/// something from the source is allocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintDiagnostic {
    pub rule: &'static str,
    pub category: Category,
    pub severity: Severity,
    pub message: Cow<'static, str>,
    pub span: Span,
    pub help: Option<Cow<'static, str>>,
    pub fix: Option<Fix>,
}

/// The driver makes each one a [`LintDiagnostic`] carrying the reporting
/// rule's name and category and its resolved severity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub message: Cow<'static, str>,
    pub span: Span,
    pub help: Option<Cow<'static, str>>,
    pub fix: Option<Fix>,
}

impl Finding {
    #[must_use]
    pub(crate) fn new<Message>(message: Message, span: Span) -> Self
    where
        Message: Into<Cow<'static, str>>,
    {
        Self { message: message.into(), span, help: None, fix: None }
    }

    #[must_use]
    pub(crate) fn with_help<Help>(mut self, help: Help) -> Self
    where
        Help: Into<Cow<'static, str>>,
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
