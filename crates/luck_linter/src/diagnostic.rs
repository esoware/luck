use alloc::borrow::Cow;

use luck_token::Span;

use crate::fix::{Fix, FixKind, RuleFixer};
use crate::rule::LintContext;

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
///
/// Its fields are private, so a fix reaches one only through
/// [`Self::with_fix`] and its checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    message: Cow<'static, str>,
    span: Span,
    help: Option<Cow<'static, str>>,
    fix: Option<Fix>,
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

    /// The fix `build` makes, of `kind`, which must be one kind. `build` runs
    /// only when the host applies fixes of `kind`, so a plain lint builds
    /// none.
    ///
    /// A fix whose edits replace a comment that the replacement doesn't
    /// carry is dropped, since no fix may delete what the author wrote
    /// beside the code.
    #[must_use]
    pub(crate) fn with_fix<Build, Built>(
        mut self,
        context: &LintContext<'_>,
        kind: FixKind,
        build: Build,
    ) -> Self
    where
        Build: FnOnce(RuleFixer) -> Built,
        Built: Into<Option<Fix>>,
    {
        debug_assert!(
            kind == FixKind::Fix || kind == FixKind::Suggestion,
            "a fix has exactly one kind, not {kind:?}"
        );
        if context.fixes.contains(kind) {
            self.fix =
                build(RuleFixer::new(kind)).into().filter(|fix| keeps_comments(fix, context));
        }
        self
    }

    /// [`Self::with_fix`] for a [`FixKind::Fix`] that removes all or part of
    /// a directive comment, which the comment check would drop.
    #[must_use]
    pub(crate) fn with_directive_fix<Build>(
        mut self,
        context: &LintContext<'_>,
        build: Build,
    ) -> Self
    where
        Build: FnOnce(RuleFixer) -> Fix,
    {
        if context.fixes.contains(FixKind::Fix) {
            self.fix = Some(build(RuleFixer::new(FixKind::Fix)));
        }
        self
    }

    pub(crate) const fn span(&self) -> Span {
        self.span
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

/// Whether each edit's replacement holds, in order, every comment in the
/// span it replaces.
fn keeps_comments(fix: &Fix, context: &LintContext<'_>) -> bool {
    fix.edits.iter().all(|edit| {
        let mut unmatched: &str = &edit.replacement;
        for comment in context.comments_within(edit.span) {
            let text = comment.span.text(context.source);
            let Some(at) = unmatched.find(text) else {
                return false;
            };
            unmatched = unmatched.get(at.saturating_add(text.len())..).unwrap_or_default();
        }
        true
    })
}
