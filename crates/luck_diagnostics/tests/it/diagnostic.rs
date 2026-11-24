use luck_diagnostics::{Diagnostic, Label, Severity, Tag};
use luck_token::Span;

#[test]
fn a_diagnostic_is_one_pointer() {
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
