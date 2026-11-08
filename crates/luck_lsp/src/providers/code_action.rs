//! Code actions: each diagnostic's fix, when it has one, and a
//! disable-for-this-line comment, plus `source.fixAll.luck` for the whole
//! file.

use alloc::sync::Arc;

use luck_linter::{Fix, LintDiagnostic};
use luck_token::Span;
use rustc_hash::FxHashMap;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionResponse, NumberOrString, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::document::DocumentState;
use crate::settings::ProjectSettings;

/// The actions for `params`, built from `document`'s `lints`.
pub(crate) fn code_action(
    document: &DocumentState,
    params: &CodeActionParams,
    lints: &[LintDiagnostic],
) -> CodeActionResponse {
    let uri = &params.text_document.uri;
    let mut lints_by_rule: FxHashMap<&str, Vec<&LintDiagnostic>> = FxHashMap::default();
    for lint in lints {
        lints_by_rule.entry(lint.rule).or_default().push(lint);
    }

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    for diagnostic in &params.context.diagnostics {
        let Some(NumberOrString::String(rule)) = &diagnostic.code else {
            continue;
        };
        let Some(matching_lints) = lints_by_rule.get(rule.as_str()) else {
            continue;
        };
        for lint in matching_lints {
            let lint_range = document.range(lint.span);
            if lint_range.end < params.range.start || lint_range.start > params.range.end {
                continue;
            }
            if let Some(fix) = &lint.fix {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: fix.description.clone(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: Some(workspace_edit(uri, fix_edits(document, fix))),
                    is_preferred: Some(true),
                    disabled: None,
                    command: None,
                    data: None,
                }));
            }
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Disable {rule} for this line"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diagnostic.clone()]),
                edit: Some(workspace_edit(uri, vec![disable_for_line(document, lint.span, rule)])),
                is_preferred: Some(false),
                disabled: None,
                command: None,
                data: None,
            }));
        }
    }

    if should_offer_fix_all(&params.context) {
        let edits = all_fix_edits(document, lints);
        if !edits.is_empty() {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Fix all auto-fixable problems".to_owned(),
                kind: Some(CodeActionKind::SOURCE_FIX_ALL),
                diagnostics: None,
                edit: Some(workspace_edit(uri, edits)),
                is_preferred: None,
                disabled: None,
                command: None,
                data: None,
            }));
        }
    }

    actions
}

fn fix_edits(document: &DocumentState, fix: &Fix) -> Vec<TextEdit> {
    fix.edits
        .iter()
        .map(|edit| TextEdit {
            range: document.range(edit.span),
            new_text: edit.replacement.clone(),
        })
        .collect()
}

/// Every fix's edits, in lint order.
fn all_fix_edits(document: &DocumentState, lints: &[LintDiagnostic]) -> Vec<TextEdit> {
    lints
        .iter()
        .filter_map(|lint| lint.fix.as_ref())
        .flat_map(|fix| fix_edits(document, fix))
        .collect()
}

fn disable_for_line(document: &DocumentState, span: Span, rule: &str) -> TextEdit {
    let line_start = Position { line: document.position(span.start).line, character: 0 };
    TextEdit {
        range: Range { start: line_start, end: line_start },
        new_text: format!("-- luck: allow({rule})\n"),
    }
}

fn workspace_edit(uri: &Url, edits: Vec<TextEdit>) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some([(uri.clone(), edits)].into()),
        document_changes: None,
        change_annotations: None,
    }
}

fn should_offer_fix_all(context: &CodeActionContext) -> bool {
    context.only.as_ref().is_none_or(|kinds| {
        kinds.iter().any(|kind| {
            kind == &CodeActionKind::SOURCE_FIX_ALL
                || kind.as_str() == "source.fixAll.luck"
                || kind == &CodeActionKind::SOURCE
        })
    })
}

/// Every available fix across the open documents, for the
/// `luck/fixAllWorkspace` custom request.
pub(crate) fn fix_all_open(
    documents: &[(Url, Arc<DocumentState>, Arc<ProjectSettings>)],
) -> WorkspaceEdit {
    let changes = documents
        .iter()
        .filter_map(|(uri, document, settings)| {
            // The opt-in gate keeps fix-all away from diagnostics that were never
            // published.
            let edits = all_fix_edits(document, &document.lint(&settings.lint_config));
            (!edits.is_empty()).then(|| (uri.clone(), edits))
        })
        .collect();
    WorkspaceEdit { changes: Some(changes), document_changes: None, change_annotations: None }
}
