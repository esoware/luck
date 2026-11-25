use std::borrow::Cow;
use std::ops::Deref;

use luck_diagnostics::{Diagnostic, Label, Severity, Tag};
use luck_token::Span;
use memchr::memmem;

use crate::context::ContextHost;
use crate::fix::{Fix, FixKind, RuleFixer};
use crate::rules::RuleId;

/// What the linter reports: a diagnostic, coded with the name of the rule
/// that found it or E008 for a parse error, and its fix, when the rule
/// offers one of a kind the host asked for.
///
/// It derefs to its diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub diagnostic: Diagnostic,
    pub fix: Option<Fix>,
}

impl Message {
    /// Where the problem is, which every lint and parse error has.
    pub fn span(&self) -> Span {
        self.diagnostic.span.unwrap_or_default()
    }
}

impl Deref for Message {
    type Target = Diagnostic;

    fn deref(&self) -> &Self::Target {
        &self.diagnostic
    }
}

/// What a rule found, which the linter makes a [`Message`] coded with the
/// rule's name, at the severity it resolves, once no directive drops it, so
/// a dropped one never builds its diagnostic.
///
/// Its fields are private, so a fix reaches one only through
/// [`Self::with_fix`] and its checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    message: Cow<'static, str>,
    span: Span,
    labels: Vec<Label>,
    help: Option<Cow<'static, str>>,
    tag: Option<Tag>,
    fix: Option<Fix>,
}

impl Finding {
    pub(crate) fn new<Text>(message: Text, span: Span) -> Self
    where
        Text: Into<Cow<'static, str>>,
    {
        Self { message: message.into(), span, labels: Vec::new(), help: None, tag: None, fix: None }
    }

    /// Adds another place the finding is about, such as the first of two
    /// definitions, and what `message` says of it.
    pub(crate) fn with_label<Text>(mut self, span: Span, message: Text) -> Self
    where
        Text: Into<Cow<'static, str>>,
    {
        self.labels.push(Label { span, message: message.into() });
        self
    }

    pub(crate) fn with_help<Help>(mut self, help: Help) -> Self
    where
        Help: Into<Cow<'static, str>>,
    {
        self.help = Some(help.into());
        self
    }

    /// Marks the code at the finding's span as doing nothing, or as a use of
    /// something deprecated, for an editor to draw it so.
    pub(crate) const fn with_tag(mut self, tag: Tag) -> Self {
        self.tag = Some(tag);
        self
    }

    /// The fix `build` makes, of `kind`, which must be one kind. `build` runs
    /// only when the host applies fixes of `kind`, so a plain lint builds
    /// none.
    ///
    /// A fix whose edits replace a comment that the replacement doesn't
    /// carry is dropped, since no fix may delete what the author wrote
    /// beside the code.
    pub(crate) fn with_fix<Build, Built>(
        mut self,
        context: &ContextHost<'_>,
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
    pub(crate) fn with_directive_fix<Build>(
        mut self,
        context: &ContextHost<'_>,
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

    pub(crate) fn into_message(self, rule: RuleId, severity: Severity) -> Message {
        let Self { message, span, labels, help, tag, fix } = self;
        let mut diagnostic =
            Diagnostic::lint(rule.name(), severity, span, message).with_url(rule.docs_url());
        diagnostic.labels = labels;
        diagnostic.help = help;
        diagnostic.tag = tag;
        Message { diagnostic, fix }
    }
}

/// Whether each edit's replacement holds, in order, every comment in the
/// span it replaces.
fn keeps_comments(fix: &Fix, context: &ContextHost<'_>) -> bool {
    fix.edits.iter().all(|edit| {
        let mut unmatched: &[u8] = &edit.replacement;
        for comment in context.comments_within(edit.span) {
            let text = comment.span.text(context.source);
            let Some(at) = memmem::find(unmatched, text) else {
                return false;
            };
            unmatched = &unmatched[at + text.len()..];
        }
        true
    })
}
