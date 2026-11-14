use alloc::sync::Arc;
use core::ops;

use luck_ast::{Expression, LastStatement, Statement, Visitor};
use luck_linter::{Fix, LintDiagnostic};
use luck_token::{Span, index_to_offset, is_lua_whitespace, line_breaks};
use rustc_hash::FxHashMap;
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionResponse, Diagnostic, NumberOrString, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::DocumentState;
use crate::settings::ProjectSettings;

/// A client drops an action that isn't inside the kind it asked for, and this
/// most specific kind is inside `source`, `source.fixAll`, and itself.
const FIX_ALL_KIND_NAME: &str = "source.fixAll.luck";

pub(crate) const FIX_ALL_KIND: CodeActionKind = CodeActionKind::new(FIX_ALL_KIND_NAME);

pub(crate) fn code_action(
    document: &DocumentState,
    params: &CodeActionParams,
    lints: &[LintDiagnostic],
) -> CodeActionResponse {
    let uri = &params.text_document.uri;
    let lint_of_diagnostic: FxHashMap<(&str, Span), &LintDiagnostic> =
        lints.iter().map(|lint| ((lint.rule, lint.span), lint)).collect();

    let matched: Vec<(&Diagnostic, &str, &LintDiagnostic)> = params
        .context
        .diagnostics
        .iter()
        .filter_map(|diag| {
            let Some(NumberOrString::String(rule)) = &diag.code else {
                return None;
            };
            if diag.range.end < params.range.start || diag.range.start > params.range.end {
                return None;
            }
            // A diagnostic carries its lint's rule and range back unchanged,
            // so the pair names the one lint it came from.
            let byte_range = document.byte_range(diag.range)?;
            let span = Span::from_indices(byte_range.start, byte_range.end);
            let lint = lint_of_diagnostic.get(&(rule.as_str(), span))?;
            Some((diag, rule.as_str(), *lint))
        })
        .collect();
    // Directives apply only after a clean parse, so none can silence a syntax
    // error.
    let disabler = document.parsed.errors.is_empty().then(|| {
        Disabler::new(
            document,
            &matched.iter().map(|(_, _, lint)| lint.span.start).collect::<Vec<_>>(),
        )
    });

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    for (index, &(diag, rule, lint)) in matched.iter().enumerate() {
        if let Some(fix) = &lint.fix {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: fix.description.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(workspace_edit(uri, fix_edits(document, fix))),
                is_preferred: Some(true),
                disabled: None,
                command: None,
                data: None,
            }));
        }
        if let Some(edit) = disabler.as_ref().and_then(|active| active.edit(index, rule)) {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Disable {rule} for this statement"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(workspace_edit(uri, vec![edit])),
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
                kind: Some(FIX_ALL_KIND),
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

/// LSP forbids overlapping edits in one change list, and a dropped fix
/// reappears on the next pass.
fn all_fix_edits(document: &DocumentState, lints: &[LintDiagnostic]) -> Vec<TextEdit> {
    luck_linter::non_overlapping_edits(lints)
        .into_iter()
        .map(|edit| TextEdit {
            range: document.range(edit.span),
            new_text: edit.replacement.clone(),
        })
        .collect()
}

/// Where to put a comment that disables a rule for each lint of a request.
///
/// One walk finds the innermost statement holding every lint, so a request
/// over a file's every lint stays linear in its size.
struct Disabler<'document> {
    document: &'document DocumentState,
    /// The end of the innermost statement holding each lint, in the lints'
    /// order, or `None` for a lint no statement holds.
    statement_ends: Vec<Option<u32>>,
    /// Where each line's trailing whitespace starts, by the line break that
    /// ends it, or the text's end for the last line.
    line_tails: Vec<(u32, u32)>,
}

impl<'document> Disabler<'document> {
    fn new(document: &'document DocumentState, lint_starts: &[u32]) -> Self {
        let mut order: Vec<usize> = (0..lint_starts.len()).collect();
        order.sort_by_key(|&index| lint_starts.get(index).copied());
        let sorted_starts =
            order.iter().filter_map(|&index| lint_starts.get(index).copied()).collect();
        let mut finder =
            StatementFinder { starts: sorted_starts, ends: vec![None; lint_starts.len()] };
        finder.visit_block(&document.parsed.block);
        let mut statement_ends = vec![None; lint_starts.len()];
        for (&index, end) in order.iter().zip(finder.ends) {
            if let Some(slot) = statement_ends.get_mut(index) {
                *slot = end;
            }
        }
        Self { document, statement_ends, line_tails: line_tails(document) }
    }

    /// A comment that disables `rule` for the innermost statement holding the
    /// lint at `index`.
    ///
    /// It trails the statement, the one place a directive covers exactly that
    /// statement and no text can swallow it. A line comment fits only at the
    /// end of a line, and one before a following statement or comment needs
    /// the block form.
    fn edit(&self, index: usize, rule: &str) -> Option<TextEdit> {
        let statement_end = (*self.statement_ends.get(index)?)?;
        let line = self.line_tails.partition_point(|&(line_end, _)| line_end < statement_end);
        let is_line_end =
            self.line_tails.get(line).is_none_or(|&(_, blank_from)| blank_from <= statement_end);
        let new_text = if is_line_end {
            format!(" -- luck: allow({rule})")
        } else {
            format!(" --[[ luck: allow({rule}) ]]")
        };
        let position = self.document.position(statement_end);
        Some(TextEdit { range: Range::new(position, position), new_text })
    }
}

/// Each line of `document`, as its Lua dialect breaks lines, as the offset its
/// break starts at and the offset its trailing whitespace starts at.
fn line_tails(document: &DocumentState) -> Vec<(u32, u32)> {
    let text = document.text();
    let mut tails = Vec::new();
    let mut line_start = 0;
    let breaks = line_breaks(text, document.lua_version).map(|line_break| line_break.start);
    for line_end in breaks.chain([text.len()]) {
        let line = text.as_bytes().get(line_start..line_end).unwrap_or_default();
        let blank_length = line.iter().rev().take_while(|&&b| is_lua_whitespace(b)).count();
        tails.push((
            index_to_offset(line_end),
            index_to_offset(line_end.saturating_sub(blank_length)),
        ));
        line_start = line_end;
    }
    tails
}

/// Finds the innermost statement holding each of `starts`, which are sorted,
/// walking only into what holds one.
struct StatementFinder {
    starts: Vec<u32>,
    ends: Vec<Option<u32>>,
}

impl StatementFinder {
    /// The indexes of the starts inside `span`.
    fn held(&self, span: Span) -> ops::Range<usize> {
        let first = self.starts.partition_point(|&start| start < span.start);
        let end = self.starts.partition_point(|&start| start < span.end);
        first..end.max(first)
    }

    /// Whether a statement at `span` holds a start, the same test a
    /// directive's range applies to a lint's start, which it marks as held
    /// there.
    fn holds(&mut self, span: Span) -> bool {
        let held = self.held(span);
        let is_held = !held.is_empty();
        for end in self.ends.get_mut(held).into_iter().flatten() {
            *end = Some(span.end);
        }
        is_held
    }
}

impl<'ast> Visitor<'ast> for StatementFinder {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if self.holds(stmt.span()) {
            self.walk_stmt(stmt);
        }
    }

    fn visit_last_stmt(&mut self, last: &'ast LastStatement) {
        if self.holds(last.span()) {
            self.walk_last_stmt(last);
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if !self.held(expr.span()).is_empty() {
            self.walk_expr(expr);
        }
    }
}

fn workspace_edit(uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some([(uri.clone(), edits)].into()),
        document_changes: None,
        change_annotations: None,
    }
}

fn should_offer_fix_all(context: &CodeActionContext) -> bool {
    context.only.as_ref().is_none_or(|kinds| {
        kinds.iter().any(|kind| {
            // Kinds nest on `.`, and the empty kind holds them all.
            let requested = kind.as_str();
            requested.is_empty()
                || FIX_ALL_KIND_NAME
                    .strip_prefix(requested)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
    })
}

pub(crate) fn fix_all_open(
    documents: &[(Uri, Arc<DocumentState>, Arc<ProjectSettings>)],
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
    use luck_core::LuaTarget;
    use luck_linter::{Category, LintConfig, RuleLevel, Severity, TextEdit as LinterTextEdit};
    use rustc_hash::FxHashSet;
    use tower_lsp_server::ls_types::{
        PartialResultParams, Position, TextDocumentIdentifier, WorkDoneProgressParams,
    };

    use super::*;
    use crate::providers::diagnostics;

    fn uri() -> Uri {
        "file:///main.lua".parse().expect("valid URI")
    }

    fn config_enabling(rule: &str) -> LintConfig {
        let mut config = LintConfig::default();
        config.categories.set_level(Category::Correctness, RuleLevel::Off);
        config.rules.set_level(rule, RuleLevel::Warn);
        config
    }

    fn document(text: &str) -> DocumentState {
        DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default())
    }

    fn params(
        range: Range,
        diags: Vec<Diagnostic>,
        only: Option<Vec<CodeActionKind>>,
    ) -> CodeActionParams {
        CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri() },
            range,
            context: CodeActionContext { diagnostics: diags, only, trigger_kind: None },
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

    fn applied(document: &DocumentState, mut edits: Vec<TextEdit>) -> String {
        edits.sort_by_key(|edit| edit.range.start);
        let mut text = document.text().to_owned();
        for edit in edits.iter().rev() {
            let byte_range = document.byte_range(edit.range).expect("the edit fits the document");
            text.replace_range(byte_range, &edit.new_text);
        }
        text
    }

    fn disabled(text: &str, config: &LintConfig, lint_index: usize) -> String {
        let document = document(text);
        let lints = document.lint(config);
        let lint = lints.get(lint_index).expect("the lint fired");
        let disabler = Disabler::new(&document, &[lint.span.start]);
        let edit = disabler.edit(0, lint.rule).expect("a statement holds it");
        applied(&document, vec![edit])
    }

    #[test]
    fn fix_all_action_has_the_kind_the_client_requests() {
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
    fn fix_all_edits_do_not_overlap() {
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
    fn two_diagnostics_of_one_rule_yield_one_fix_each() {
        let document = document("local x = nil local y = nil\n");
        let lints = document.lint(&config_enabling("redundant_nil_init"));
        let diags = diagnostics::to_lsp(&document, &lints);
        let range = Range::new(Position::new(0, 0), Position::new(0, 27));
        let actions = code_action(&document, &params(range, diags, None), &lints);
        let fix_count = actions
            .iter()
            .filter(|action| only_action(action).title.starts_with("drop redundant"))
            .count();
        assert_eq!(fix_count, 2, "one fix per diagnostic, not one per diagnostic per lint");
    }

    #[test]
    fn no_disable_action_for_a_parse_error() {
        // Directives apply only after a clean parse, so a comment can't
        // silence a syntax error.
        let document = document("local x =\n");
        let config = config_enabling("redundant_nil_init");
        let lints = document.lint(&config);
        let diags = diagnostics::to_lsp(&document, &lints);
        let range = Range::new(Position::new(0, 0), Position::new(1, 0));
        let actions = code_action(&document, &params(range, diags, None), &lints);
        let titles: Vec<&str> =
            actions.iter().map(|action| only_action(action).title.as_str()).collect();
        assert!(
            !titles.iter().any(|title| title.contains("parse_error")),
            "offering to disable an unsuppressible error: {titles:?}"
        );
    }

    #[test]
    fn disable_comment_suppresses_a_later_statement_on_the_line() {
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
    fn disable_comment_stays_out_of_a_long_string() {
        let text = "local s = [[\nx]] local y = nil\n";
        let config = config_enabling("redundant_nil_init");
        let edited = disabled(text, &config, 0);
        assert!(
            edited.contains("[[\nx]]"),
            "the comment must not rewrite the string's contents: {edited:?}"
        );
    }

    #[test]
    fn disable_comment_does_not_silence_the_next_statement() {
        let text = "local t = {\n  first_missing,\n}\nlocal u = second_missing\n";
        let config = config_enabling("undefined_variable");
        let edited = disabled(text, &config, 0);
        let remaining = document(&edited).lint(&config);
        assert!(
            remaining.iter().any(|lint| lint.message.contains("second_missing")),
            "\"for this line\" must leave the next statement flagged: {edited:?}"
        );
    }

    #[test]
    fn disable_comment_before_a_later_statement_silences_only_its_own() {
        let text = "local x = nil local y = nil\n";
        let config = config_enabling("redundant_nil_init");
        let edited = disabled(text, &config, 0);
        assert!(edited.contains("--[[ luck: allow(redundant_nil_init) ]]"), "{edited:?}");
        let reparsed = document(&edited);
        assert!(reparsed.parsed.errors.is_empty(), "{:?}", reparsed.parsed.errors);
        let remaining = reparsed.lint(&config);
        assert_eq!(remaining.len(), 1, "the later statement stays flagged: {remaining:?}");
    }

    fn lint_with_fix(edits: Vec<LinterTextEdit>) -> LintDiagnostic {
        LintDiagnostic {
            rule: "test_rule",
            category: Category::Style,
            severity: Severity::Warning,
            message: "bad".to_owned(),
            span: Span::new(0, 0),
            help: None,
            fix: Some(Fix { description: "fix".to_owned(), edits }),
        }
    }

    fn linter_edit(start: u32, end: u32) -> LinterTextEdit {
        LinterTextEdit { span: Span::new(start, end), replacement: "x".to_owned() }
    }

    #[test]
    fn a_fix_that_overlaps_a_later_one_drops_whole() {
        let document = document("0123456789\n");
        let fixes = [
            lint_with_fix(vec![linter_edit(0, 1), linter_edit(2, 6)]),
            lint_with_fix(vec![linter_edit(5, 8)]),
            lint_with_fix(vec![linter_edit(9, 9), linter_edit(9, 9)]),
        ];
        let edits = all_fix_edits(&document, &fixes);
        let starts: Vec<u32> = edits.iter().map(|edit| edit.range.start.character).collect();
        assert_eq!(starts, vec![5, 9, 9], "the two-part fix is kept or dropped whole: {edits:?}");
    }
}
