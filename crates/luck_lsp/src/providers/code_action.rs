//! Code actions: per-diagnostic auto-fix, source.fixAll, and rule-disable
//! comment-insertion actions.

use std::collections::HashMap;

use luck_core::config::LuckConfig;
use luck_linter::diagnostic::LintDiagnostic;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionResponse, Diagnostic, NumberOrString, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::backend::DocumentState;
use crate::config::ProjectSettings;
use crate::line_index::LineIndex;

#[must_use]
pub fn code_action(
    doc: &DocumentState,
    settings: &ProjectSettings,
    uri: &Url,
    params: &CodeActionParams,
    cached_lints: Option<std::sync::Arc<Vec<LintDiagnostic>>>,
) -> CodeActionResponse {
    let mut actions: Vec<CodeActionOrCommand> = Vec::new();

    // The same lint pass that produced the published diagnostics (through
    // the opt-in gate, so we never offer fixes the user wasn't shown).
    // Served from the backend's cache when current; recomputed only on a
    // version mismatch - VS Code fires codeAction on nearly every cursor
    // move, and each request used to re-run all 47 rules.
    let lint_config = settings.effective_lint_config();
    let all_diags: std::sync::Arc<Vec<LintDiagnostic>> = match cached_lints {
        Some(cached) => cached,
        None => std::sync::Arc::new(luck_linter::lint_parsed(
            &doc.parsed,
            doc.target.lua_version(),
            doc.target.stdlib_environment(),
            &lint_config,
        )),
    };

    let by_rule: HashMap<&str, Vec<&LintDiagnostic>> =
        all_diags.iter().fold(HashMap::new(), |mut acc, d| {
            acc.entry(d.rule).or_default().push(d);
            acc
        });

    // Per-diagnostic quick-fixes for diagnostics within the cursor range.
    for diag in &params.context.diagnostics {
        let rule = diagnostic_rule(diag);
        let Some(matching) = rule.and_then(|r| by_rule.get(r)) else {
            continue;
        };
        for source_diag in matching {
            if !span_overlaps_range(source_diag.span, &params.range, &doc.line_index, &doc.text) {
                continue;
            }
            if let Some(fix) = &source_diag.fix {
                let edits = fix_to_text_edits(fix, &doc.line_index, &doc.text);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: fix.description.clone(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag.clone()]),
                    edit: Some(workspace_edit(uri, edits)),
                    is_preferred: Some(true),
                    disabled: None,
                    command: None,
                    data: None,
                }));
            }
            if let Some(rule_name) = rule {
                let disable_edit = disable_for_line(source_diag.span, rule_name, doc);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Disable {rule_name} for this line"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag.clone()]),
                    edit: Some(workspace_edit(uri, vec![disable_edit])),
                    is_preferred: Some(false),
                    disabled: None,
                    command: None,
                    data: None,
                }));
            }
        }
    }

    // source.fixAll.luck: apply every fix in the file.
    if should_offer_fix_all(&params.context) {
        let edits: Vec<TextEdit> = all_diags
            .iter()
            .filter_map(|d| d.fix.as_ref())
            .flat_map(|fix| fix_to_text_edits(fix, &doc.line_index, &doc.text))
            .collect();
        if !edits.is_empty() {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Fix all auto-fixable problems".to_string(),
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

fn diagnostic_rule(diag: &Diagnostic) -> Option<&str> {
    match &diag.code {
        Some(NumberOrString::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn span_overlaps_range(
    span: luck_token::Span,
    range: &Range,
    line_index: &LineIndex,
    source: &str,
) -> bool {
    let lsp = line_index.range(source, span.start, span.end);
    !(lsp.end.line < range.start.line
        || (lsp.end.line == range.start.line && lsp.end.character < range.start.character)
        || lsp.start.line > range.end.line
        || (lsp.start.line == range.end.line && lsp.start.character > range.end.character))
}

fn fix_to_text_edits(
    fix: &luck_linter::diagnostic::Fix,
    line_index: &LineIndex,
    source: &str,
) -> Vec<TextEdit> {
    fix.edits
        .iter()
        .map(|edit| TextEdit {
            range: line_index.range(source, edit.span.start, edit.span.end),
            new_text: edit.replacement.clone(),
        })
        .collect()
}

fn disable_for_line(span: luck_token::Span, rule: &str, doc: &DocumentState) -> TextEdit {
    let pos = doc.line_index.position(&doc.text, span.start);
    let line_start = Position {
        line: pos.line,
        character: 0,
    };
    TextEdit {
        range: Range {
            start: line_start,
            end: line_start,
        },
        new_text: format!("-- luck: allow({rule})\n"),
    }
}

fn workspace_edit(uri: &Url, edits: Vec<TextEdit>) -> WorkspaceEdit {
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

fn should_offer_fix_all(ctx: &CodeActionContext) -> bool {
    match &ctx.only {
        None => true,
        Some(kinds) => kinds.iter().any(|k| {
            k == &CodeActionKind::SOURCE_FIX_ALL
                || k.as_str() == "source.fixAll.luck"
                || k == &CodeActionKind::SOURCE
        }),
    }
}

/// Workspace-wide "fix all" - used by the `luck/fixAllWorkspace`
/// custom request. Walks every open document and returns the merged
/// edit set.
#[must_use]
pub fn fix_all_open(
    documents: &HashMap<Url, DocumentState>,
    settings_by_uri: impl Fn(&Url) -> ProjectSettings,
    _config: &LuckConfig,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for (uri, doc) in documents.iter() {
        let settings = settings_by_uri(uri);
        // Apply the opt-in gate so fix-all never touches diagnostics that were
        // never published (a project with no `lint` section shows only parse
        // errors, so fix-all must not silently rewrite for default rules).
        let lint_config = settings.effective_lint_config();
        let diags = luck_linter::lint_parsed(
            &doc.parsed,
            doc.target.lua_version(),
            doc.target.stdlib_environment(),
            &lint_config,
        );
        let edits: Vec<TextEdit> = diags
            .iter()
            .filter_map(|d| d.fix.as_ref())
            .flat_map(|fix| fix_to_text_edits(fix, &doc.line_index, &doc.text))
            .collect();
        if !edits.is_empty() {
            changes.insert(uri.clone(), edits);
        }
    }
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}
