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

/// Every available fix across `documents`, for the `luck/fixAllWorkspace`
/// custom request.
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

#[cfg(test)]
mod tests {
    use luck_core::{LuaTarget, RuleSetting};
    use luck_linter::LintConfig;
    use rustc_hash::FxHashSet;
    use tower_lsp::lsp_types::{
        Diagnostic, PartialResultParams, TextDocumentIdentifier, WorkDoneProgressParams,
    };

    use super::*;
    use crate::providers::diagnostics;

    fn uri() -> Url {
        Url::parse("file:///main.lua").expect("valid URI")
    }

    fn config_enabling(rule: &str) -> LintConfig {
        let mut config = LintConfig { disable_default_rules: true, ..LintConfig::default() };
        config
            .rule_overrides
            .insert(rule.to_owned(), RuleSetting { enabled: Some(true), severity: None });
        config
    }

    fn document(text: &str) -> DocumentState {
        DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default())
    }

    fn params(
        range: Range,
        diagnostics: Vec<Diagnostic>,
        only: Option<Vec<CodeActionKind>>,
    ) -> CodeActionParams {
        CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri() },
            range,
            context: CodeActionContext { diagnostics, only, trigger_kind: None },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        }
    }

    fn only_action(action: &CodeActionOrCommand) -> &CodeAction {
        match action {
            CodeActionOrCommand::CodeAction(code_action) => code_action,
            CodeActionOrCommand::Command(command) => panic!("expected an action: {command:?}"),
        }
    }

    /// `text` after applying `edits`, which must not overlap.
    fn applied(document: &DocumentState, mut edits: Vec<TextEdit>) -> String {
        edits.sort_by_key(|edit| edit.range.start);
        let mut text = document.text().to_owned();
        for edit in edits.iter().rev() {
            let byte_range = document.byte_range(edit.range).expect("the edit fits the document");
            text.replace_range(byte_range, &edit.new_text);
        }
        text
    }

    /// `text` with the disable comment for the rule's `lint_index`th lint
    /// applied.
    fn disabled(text: &str, config: &LintConfig, lint_index: usize) -> String {
        let document = document(text);
        let lints = document.lint(config);
        let lint = lints.get(lint_index).expect("the lint fired");
        let edit = disable_for_line(&document, lint.span, lint.rule);
        applied(&document, vec![edit])
    }

    #[test]
    fn bug_fix_all_action_has_the_kind_the_client_requests() {
        let document = document("local x = nil\n");
        let lints = document.lint(&config_enabling("redundant_nil_init"));
        let kind = CodeActionKind::new("source.fixAll.luck");
        let actions = code_action(
            &document,
            &params(Range::default(), Vec::new(), Some(vec![kind.clone()])),
            &lints,
        );
        let action = only_action(actions.first().expect("a fix-all action"));
        assert_eq!(
            action.kind,
            Some(kind),
            "clients drop an action not inside the requested kind, and `source.fixAll` isn't inside `source.fixAll.luck`"
        );
    }

    #[test]
    fn bug_fix_all_edits_do_not_overlap() {
        let document = document("return not (a == (not (b == c)))\n");
        let lints = document.lint(&config_enabling("unnecessary_negation"));
        let edits = all_fix_edits(&document, &lints);
        let mut ranges: Vec<Range> = edits.iter().map(|edit| edit.range).collect();
        ranges.sort_by_key(|range| range.start);
        for pair in ranges.windows(2) {
            if let [earlier, later] = pair {
                assert!(
                    earlier.end <= later.start,
                    "LSP forbids overlapping edits in one change list: {ranges:?}"
                );
            }
        }
    }

    #[test]
    fn bug_two_diagnostics_of_one_rule_yield_one_fix_each() {
        let document = document("local x = nil local y = nil\n");
        let lints = document.lint(&config_enabling("redundant_nil_init"));
        let diagnostics = diagnostics::to_lsp(&document, &lints);
        let range = Range::new(Position::new(0, 0), Position::new(0, 27));
        let actions = code_action(&document, &params(range, diagnostics, None), &lints);
        let fix_count = actions
            .iter()
            .filter(|action| only_action(action).title.starts_with("drop redundant"))
            .count();
        assert_eq!(fix_count, 2, "one fix per diagnostic, not one per diagnostic per lint");
    }

    #[test]
    fn bug_no_disable_action_for_a_parse_error() {
        // Directives apply only after a clean parse, so a comment can't
        // silence a syntax error.
        let document = document("local x =\n");
        let config = config_enabling("redundant_nil_init");
        let lints = document.lint(&config);
        let diagnostics = diagnostics::to_lsp(&document, &lints);
        let range = Range::new(Position::new(0, 0), Position::new(1, 0));
        let actions = code_action(&document, &params(range, diagnostics, None), &lints);
        let titles: Vec<&str> =
            actions.iter().map(|action| only_action(action).title.as_str()).collect();
        assert!(
            !titles.iter().any(|title| title.contains("parse_error")),
            "offering to disable an unsuppressible error: {titles:?}"
        );
    }

    #[test]
    fn bug_disable_comment_suppresses_a_later_statement_on_the_line() {
        let text = "local a = 1 local x = nil\n";
        let config = config_enabling("redundant_nil_init");
        let edited = disabled(text, &config, 0);
        let remaining = document(&edited).lint(&config);
        assert!(
            remaining.is_empty(),
            "the comment must silence the line: {edited:?} {remaining:?}"
        );
    }

    #[test]
    fn bug_disable_comment_stays_out_of_a_long_string() {
        let text = "local s = [[\nx]] local y = nil\n";
        let config = config_enabling("redundant_nil_init");
        let edited = disabled(text, &config, 0);
        assert!(
            edited.contains("[[\nx]]"),
            "the comment must not rewrite the string's contents: {edited:?}"
        );
    }

    #[test]
    fn bug_disable_comment_does_not_silence_the_next_statement() {
        let text = "local t = {\n  first_missing,\n}\nlocal u = second_missing\n";
        let config = config_enabling("undefined_variable");
        let edited = disabled(text, &config, 0);
        let remaining = document(&edited).lint(&config);
        assert!(
            remaining.iter().any(|lint| lint.message.contains("second_missing")),
            "\"for this line\" must leave the next statement flagged: {edited:?}"
        );
    }
}
