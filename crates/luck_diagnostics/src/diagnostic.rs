use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::ops::{Deref, DerefMut};

use luck_token::Span;

/// A problem a luck tool found: what it is, where, and what to do about it.
///
/// It names no file, so diagnostics travel with the file they're in, as
/// [`FileDiagnostics`](crate::FileDiagnostics) holds them. Its fields sit
/// behind one box, so a `Result` or a list of them holds a pointer each.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct Diagnostic {
    inner: Box<DiagnosticInner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticInner {
    /// An `E` or `W` code from a numbered constructor, such as
    /// [`Diagnostic::e008`], or the name of the lint rule that reported it.
    pub code: &'static str,
    pub severity: Severity,
    pub message: Cow<'static, str>,
    /// Where the problem is, or `None` for one with no place in its file,
    /// such as a file that can't be read.
    pub span: Option<Span>,
    /// The other places the problem is about, such as the construct a
    /// missing `end` leaves open, each with what it says of its place.
    pub labels: Vec<Label>,
    /// What to do about it.
    pub help: Option<Cow<'static, str>>,
    /// Why it's a problem, or what else is worth knowing.
    pub note: Option<Cow<'static, str>>,
    /// A page that documents the problem, as every lint rule has.
    pub url: Option<&'static str>,
    /// What the code at `span` is, which an editor can show it as.
    pub tag: Option<Tag>,
}

/// An error fails the run, and a warning doesn't, unless the run asks it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    /// The name machine-readable output spells it with.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// A place a diagnostic is about, beside the one where its problem is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: Cow<'static, str>,
}

/// What the code a diagnostic points at is, which an editor shows by how it
/// draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// Code that does nothing, such as an unused variable, which an editor
    /// fades.
    Unnecessary,
    /// A use of something deprecated, which an editor strikes through.
    Deprecated,
}

impl Tag {
    /// The name machine-readable output spells it with.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unnecessary => "unnecessary",
            Self::Deprecated => "deprecated",
        }
    }
}

impl Diagnostic {
    /// Out of line, so the box is built in one place rather than at every
    /// diagnostic a tool can report.
    #[cold]
    #[inline(never)]
    pub(crate) fn new(
        code: &'static str,
        severity: Severity,
        message: Cow<'static, str>,
        span: Option<Span>,
    ) -> Self {
        Self {
            inner: Box::new(DiagnosticInner {
                code,
                severity,
                message,
                span,
                labels: Vec::new(),
                help: None,
                note: None,
                url: None,
                tag: None,
            }),
        }
    }

    /// What lint rule `rule` found at `span`, which carries the rule's name
    /// as its code.
    pub fn lint<Message>(
        rule: &'static str,
        severity: Severity,
        span: Span,
        message: Message,
    ) -> Self
    where
        Message: Into<Cow<'static, str>>,
    {
        Self::new(rule, severity, message.into(), Some(span))
    }

    /// Adds a place the problem is about, and what `message` says of it.
    pub fn with_label<Message>(mut self, span: Span, message: Message) -> Self
    where
        Message: Into<Cow<'static, str>>,
    {
        self.inner.labels.push(Label { span, message: message.into() });
        self
    }

    pub fn with_help<Help>(mut self, help: Help) -> Self
    where
        Help: Into<Cow<'static, str>>,
    {
        self.inner.help = Some(help.into());
        self
    }

    pub fn with_note<Note>(mut self, note: Note) -> Self
    where
        Note: Into<Cow<'static, str>>,
    {
        self.inner.note = Some(note.into());
        self
    }

    pub fn with_url(mut self, url: &'static str) -> Self {
        self.inner.url = Some(url);
        self
    }

    pub fn with_tag(mut self, tag: Tag) -> Self {
        self.inner.tag = Some(tag);
        self
    }

    pub fn is_error(&self) -> bool {
        self.inner.severity == Severity::Error
    }
}

impl Deref for Diagnostic {
    type Target = DiagnosticInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Diagnostic {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.inner.message)
    }
}

impl Error for Diagnostic {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_one_pointer() {
        assert_eq!(size_of::<Diagnostic>(), size_of::<usize>());
        assert_eq!(size_of::<Result<(), Diagnostic>>(), size_of::<usize>());
    }

    #[test]
    fn builders_fill_their_fields() {
        let diag = Diagnostic::lint("rule", Severity::Warning, Span::new(4, 5), "found")
            .with_label(Span::new(0, 1), "here")
            .with_help("do this")
            .with_note("because")
            .with_url("https://example.com")
            .with_tag(Tag::Unnecessary);
        assert_eq!(
            (diag.code, diag.severity, diag.span),
            ("rule", Severity::Warning, Some(Span::new(4, 5)))
        );
        assert_eq!(diag.labels, [Label { span: Span::new(0, 1), message: "here".into() }]);
        assert_eq!(diag.help.as_deref(), Some("do this"));
        assert_eq!(diag.note.as_deref(), Some("because"));
        assert_eq!(diag.url, Some("https://example.com"));
        assert_eq!(diag.tag, Some(Tag::Unnecessary));
        assert!(!diag.is_error());
        assert_eq!(diag.to_string(), "found");
    }
}
