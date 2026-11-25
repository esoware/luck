use std::fmt::Write as _;

use ls_types::{
    CodeDescription, Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    DiagnosticTag, Location, NumberOrString, Range, Uri,
};
use luck_diagnostics::{Diagnostic, Severity, Tag};
use luck_linter::Message;
use luck_token::Span;

use crate::document::DocumentState;
use crate::line_index::{LineIndex, PositionEncoding};

/// The source string in `Diagnostic.source`; VS Code shows it in tooltips.
const DIAGNOSTIC_SOURCE: &str = "luck";

/// The diagnostics of the document at `uri`: `lints`, sorted by where they
/// start, as the linter returns them, and then what stops each of its
/// requires from resolving, which the resolver reads the disk for.
pub(crate) fn to_lsp(uri: &Uri, document: &DocumentState, lints: &[Message]) -> Vec<LspDiagnostic> {
    let unresolved = unresolved(document);
    let mut positions = document.positions();
    lints
        .iter()
        .map(|message| &message.diagnostic)
        .chain(&unresolved)
        .map(|diag| converted(uri, diag, |span| positions.range(span)))
        .collect()
}

/// What `luck build` reports of how `document`'s requires resolve.
///
/// That's each that resolves to no module or more than one, or through a
/// shadowed `@self`. A document the project leaves out, one that doesn't
/// parse, and one with no file have none.
fn unresolved(document: &DocumentState) -> Vec<Diagnostic> {
    if !document.settings().is_included || !document.parse().errors.is_empty() {
        return Vec::new();
    }
    let requires = document.record().requires().iter();
    requires
        .filter_map(|require| match document.resolve(require)? {
            Ok(resolution) => resolution.to_diagnostic(require.span),
            Err(error) => Some(error.to_diagnostic(require.span, &require.module)),
        })
        .collect()
}

/// The diagnostics in `text`, the config file at `uri`, which isn't open,
/// such as the problem that stops a `luck.json` from loading.
pub(crate) fn config_to_lsp(
    uri: &Uri,
    diags: &[Diagnostic],
    text: &str,
    encoding: PositionEncoding,
) -> Vec<LspDiagnostic> {
    let index = LineIndex::new(text, encoding);
    diags.iter().map(|diag| converted(uri, diag, |span| index.range(text, span))).collect()
}

/// LSP has no place for help or a note, so they follow the message. A
/// diagnostic with no place in its file sits at its start.
fn converted<RangeOf>(uri: &Uri, diag: &Diagnostic, mut range_of: RangeOf) -> LspDiagnostic
where
    RangeOf: FnMut(Span) -> Range,
{
    let mut message = diag.message.to_string();
    for (heading, extra) in [("help", &diag.help), ("note", &diag.note)] {
        if let Some(text) = extra {
            let _ = write!(message, "\n\n{heading}: {text}");
        }
    }
    let range = range_of(diag.span.unwrap_or_default());
    let related_information = (!diag.labels.is_empty()).then(|| {
        diag.labels
            .iter()
            .map(|label| DiagnosticRelatedInformation {
                location: Location { uri: uri.clone(), range: range_of(label.span) },
                message: label.message.to_string(),
            })
            .collect()
    });
    LspDiagnostic {
        range,
        severity: Some(match diag.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(diag.code.to_owned())),
        code_description: diag
            .url
            .and_then(|url| url.parse().ok())
            .map(|href| CodeDescription { href }),
        source: Some(DIAGNOSTIC_SOURCE.to_owned()),
        message,
        related_information,
        tags: diag.tag.map(|tag| {
            vec![match tag {
                Tag::Unnecessary => DiagnosticTag::UNNECESSARY,
                Tag::Deprecated => DiagnosticTag::DEPRECATED,
            }]
        }),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use ls_types::Position;
    use luck_linter::{Category, LintConfig, RuleId, RuleLevel};

    use super::*;
    use crate::fixtures::{document, document_linted_by, uri};

    fn convert(diagnostic: Diagnostic) -> LspDiagnostic {
        let document = document("local x = 1");
        let mut diags = to_lsp(&uri(), &document, &[Message { diagnostic, fix: None }]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        diags.remove(0)
    }

    /// `text`'s lints under the one rule `rule`, converted.
    fn linted(text: &str, rule: RuleId) -> Vec<LspDiagnostic> {
        let mut config = LintConfig::default();
        config.categories.set_level(Category::Correctness, RuleLevel::Off);
        config.rules.set_level(rule, RuleLevel::Warn);
        let document = document_linted_by(text, config);
        let lints = document.lint(&|| false).expect("nothing cancels it");
        to_lsp(&uri(), &document, &lints)
    }

    #[test]
    fn maps_error_severity_and_adds_nothing_the_lint_lacks() {
        let diag = convert(Diagnostic::lint("test_rule", Severity::Error, Span::new(6, 7), "bad"));
        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diag.message, "bad");
        assert_eq!(diag.code, Some(NumberOrString::String("test_rule".to_owned())));
        assert_eq!(
            (diag.related_information, diag.tags, diag.code_description),
            (None, None, None)
        );
    }

    #[test]
    fn maps_warning_severity_and_appends_help_and_note() {
        let diag = convert(
            Diagnostic::lint("warn_rule", Severity::Warning, Span::new(0, 5), "stylistic issue")
                .with_help("rename it")
                .with_note("names matter"),
        );
        assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            diag.message,
            "\
stylistic issue

help: rename it

note: names matter"
        );
    }

    #[test]
    fn sends_labels_as_related_locations_in_the_same_document() {
        let [diag] = <[LspDiagnostic; 1]>::try_from(linted(
            "\
local x = 1
do local x = 2 print(x) end
print(x)",
            RuleId::Shadowing,
        ))
        .expect("one lint");
        let related = diag.related_information.expect("the outer variable");
        assert_eq!(related.len(), 1, "{related:?}");
        assert_eq!(related[0].location.uri, uri());
        assert_eq!(related[0].location.range, Range::new(Position::new(0, 6), Position::new(0, 7)));
        assert_eq!(related[0].message, "outer variable defined here");
    }

    #[test]
    fn tags_unnecessary_and_deprecated_code() {
        let unused = linted("local unused = 1", RuleId::UnusedVariable);
        assert_eq!(unused[0].tags, Some(vec![DiagnosticTag::UNNECESSARY]), "{unused:?}");
        let deprecated = linted("return table.getn(t)", RuleId::Deprecated);
        assert_eq!(deprecated[0].tags, Some(vec![DiagnosticTag::DEPRECATED]), "{deprecated:?}");
    }

    #[test]
    fn links_each_lint_to_its_rule() {
        let diags = linted("local unused = 1", RuleId::UnusedVariable);
        let href = diags[0].code_description.as_ref().map(|description| description.href.as_str());
        assert!(
            href.is_some_and(
                |link| link.ends_with("/crates/luck_linter/src/rules/unused_variable.rs")
            ),
            "{href:?}"
        );
    }
}
