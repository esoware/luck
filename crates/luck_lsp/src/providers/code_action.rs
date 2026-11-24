use core::iter;

use ls_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionResponse, Diagnostic, NumberOrString, Range, TextEdit, Uri, WorkspaceEdit,
};
use luck_ast::{GetSpan as _, Statement};
use luck_linter::{Fix, FixKind, Message};
use luck_token::{Span, has_line_break, is_lua_whitespace, offset_to_index};
use rustc_hash::FxHashMap;

use crate::document::DocumentState;

/// A client drops an action that isn't inside the kind it asked for, and this
/// most specific kind is inside `source`, `source.fixAll`, and itself.
const FIX_ALL_KIND_NAME: &str = "source.fixAll.luck";

pub(crate) const FIX_ALL_KIND: CodeActionKind = CodeActionKind::new(FIX_ALL_KIND_NAME);

pub(crate) fn code_action(
    document: &DocumentState,
    params: &CodeActionParams,
    lints: &[Message],
) -> CodeActionResponse {
    let uri = &params.text_document.uri;
    let requested: Vec<(&Diagnostic, &str, Span)> = params
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
            let byte_range = document.byte_range(diag.range)?;
            Some((diag, rule.as_str(), Span::from_indices(byte_range.start, byte_range.end)))
        })
        .collect();
    let lints_by_key =
        lints_starting_at(lints, requested.iter().map(|&(_, _, span)| span.start).collect());
    // A diagnostic carries its lint's rule and range back unchanged, so the
    // pair names the one lint it came from.
    let matched: Vec<(&Diagnostic, &str, &Message)> = requested
        .into_iter()
        .filter_map(|(diag, rule, span)| Some((diag, rule, *lints_by_key.get(&(span, rule))?)))
        .collect();
    // Directives apply only after a clean parse, so none can silence a syntax
    // error.
    let can_disable = document.parse().errors.is_empty();

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    for &(diag, rule, lint) in &matched {
        if let Some(fix) = &lint.fix {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: fix.message.clone().into_owned(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(workspace_edit(uri, fix_edits(document, fix))),
                // The client's auto-fix applies a preferred action unasked,
                // which a suggestion never is.
                is_preferred: Some(fix.kind == FixKind::Fix),
                disabled: None,
                command: None,
                data: None,
            }));
        }
        if can_disable && let Some(edit) = disable_edit(document, lint.span().start, rule) {
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

/// The last lint of each rule and span that starts at one of `starts`, from
/// `lints` in the order the linter returns them, sorted by where they start.
///
/// A request can hold every lint of a file, and many can start at one place,
/// as each comparison in a run of them does, so a lint is found by key, not
/// by a walk over the others that start there.
fn lints_starting_at(lints: &[Message], mut starts: Vec<u32>) -> FxHashMap<(Span, &str), &Message> {
    starts.sort_unstable();
    starts.dedup();
    let mut by_key = FxHashMap::default();
    for start in starts {
        let from = lints.partition_point(|lint| lint.span().start < start);
        let to = lints.partition_point(|lint| lint.span().start <= start);
        for lint in lints.get(from..to).unwrap_or_default() {
            by_key.insert((lint.span(), lint.code), lint);
        }
    }
    by_key
}

fn fix_edits(document: &DocumentState, fix: &Fix) -> Vec<TextEdit> {
    fix.edits
        .iter()
        .map(|edit| TextEdit {
            range: document.range(edit.span),
            new_text: String::from_utf8_lossy(&edit.replacement).into_owned(),
        })
        .collect()
}

/// The edits of every fix in `lints` that keeps what the code does, which
/// is all that fix-all applies, as `--fix` does.
///
/// LSP forbids overlapping edits in one change list, and a dropped fix is
/// published again once the client applies these.
fn all_fix_edits(document: &DocumentState, lints: &[Message]) -> Vec<TextEdit> {
    let mut positions = document.positions();
    luck_linter::non_overlapping_edits(lints, FixKind::Fix)
        .into_iter()
        .map(|edit| TextEdit {
            range: positions.range(edit.span),
            new_text: String::from_utf8_lossy(&edit.replacement).into_owned(),
        })
        .collect()
}

/// A comment that disables `rule` for the innermost statement holding
/// `start`, a lint's, or `None` when no statement holds it.
///
/// A statement holds a lint that starts inside it, the test a directive's
/// range applies, and the innermost is in the innermost block that has one
/// that does, so the search walks up from the node at `start` to it.
///
/// The comment trails the statement, the one place a directive covers
/// exactly that statement and no text can swallow it. A line comment fits
/// only at the end of a line, and one before a following statement or
/// comment needs the block form.
fn disable_edit(document: &DocumentState, start: u32, rule: &str) -> Option<TextEdit> {
    let nodes = document.nodes();
    let innermost = nodes.node_at(start)?;
    let statement_end =
        iter::once(innermost).chain(nodes.ancestor_ids(innermost)).find_map(|id| {
            let stmts: &[Statement<'_>] = &nodes.kind(id).as_block()?.stmts;
            let stmt = stmts.get(stmts.partition_point(|stmt| stmt.span().end <= start))?;
            (stmt.span().start <= start).then(|| stmt.span().end)
        })?;
    let rest = document.text().get(offset_to_index(statement_end)..).unwrap_or_default();
    let blank_length = rest.bytes().take_while(|&b| is_lua_whitespace(b)).count();
    let blank = rest.get(..blank_length).unwrap_or_default();
    let is_line_end =
        blank_length == rest.len() || has_line_break(blank.as_bytes(), document.version());
    let new_text = if is_line_end {
        format!(" -- luck: allow({rule})")
    } else {
        format!(" --[[ luck: allow({rule}) ]]")
    };
    let position = document.position(statement_end);
    Some(TextEdit { range: Range::new(position, position), new_text })
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

/// The fixes of every document in `documents`, from the lints of the version
/// given, the same ones it publishes, so a document the project leaves out
/// has none.
pub(crate) fn fix_all<'document, I>(documents: I) -> WorkspaceEdit
where
    I: IntoIterator<Item = (&'document Uri, &'document DocumentState, &'document [Message])>,
{
    let changes = documents
        .into_iter()
        .filter_map(|(uri, document, lints)| {
            let edits = all_fix_edits(document, lints);
            (!edits.is_empty()).then(|| (uri.clone(), edits))
        })
        .collect();
    WorkspaceEdit { changes: Some(changes), document_changes: None, change_annotations: None }
}

#[cfg(test)]
mod tests {
    use ls_types::{PartialResultParams, Position, TextDocumentIdentifier, WorkDoneProgressParams};
    use luck_diagnostics::{Diagnostic as LuckDiagnostic, Severity};
    use luck_linter::{Category, LintConfig, RuleLevel, TextEdit as LinterTextEdit};
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::document_linted_by;
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

    /// `text` linted under `config`, and its lints.
    fn linted(text: &str, config: &LintConfig) -> (DocumentState, Vec<Message>) {
        let document = document_linted_by(text, LuaTarget::Lua54, config.clone());
        let lints = document.lint(&|| false).expect("nothing cancels it");
        (document, lints)
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
        let (document, lints) = linted(text, config);
        let lint = lints.get(lint_index).expect("the lint fired");
        let edit =
            disable_edit(&document, lint.span().start, lint.code).expect("a statement holds it");
        applied(&document, vec![edit])
    }

    #[test]
    fn fix_all_action_has_the_kind_the_client_requests() {
        let (document, lints) = linted("local x = nil\n", &config_enabling("redundant_nil_init"));
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
        let (document, lints) =
            linted("return not (a == (not (b == c)))\n", &config_enabling("unnecessary_negation"));
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
        let (document, lints) =
            linted("local x = nil local y = nil\n", &config_enabling("redundant_nil_init"));
        let diags = diagnostics::to_lsp(&uri(), &document, &lints);
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
        let (document, lints) = linted("local x =\n", &config_enabling("redundant_nil_init"));
        let diags = diagnostics::to_lsp(&uri(), &document, &lints);
        let range = Range::new(Position::new(0, 0), Position::new(1, 0));
        let actions = code_action(&document, &params(range, diags, None), &lints);
        let titles: Vec<&str> =
            actions.iter().map(|action| only_action(action).title.as_str()).collect();
        assert!(
            !titles.iter().any(|title| title.starts_with("Disable")),
            "offering to disable an unsuppressible error: {titles:?}"
        );
    }

    #[test]
    fn disable_comment_trails_the_innermost_statement() {
        let text = "\
local f = function()
  local x = nil
end
";
        let config = config_enabling("redundant_nil_init");
        let edited = disabled(text, &config, 0);
        assert!(
            edited.contains("local x = nil -- luck: allow(redundant_nil_init)\n"),
            "the statement in the function holds the lint, not the one around it: {edited:?}"
        );
        let (_, remaining) = linted(&edited, &config);
        assert!(remaining.is_empty(), "{edited:?} {remaining:?}");
    }

    #[test]
    fn disable_comment_suppresses_a_later_statement_on_the_line() {
        let text = "local a = 1 local x = nil\n";
        let config = config_enabling("redundant_nil_init");
        let edited = disabled(text, &config, 0);
        let (_, remaining) = linted(&edited, &config);
        assert!(
            remaining.is_empty(),
            "the comment must silence the line: {edited:?} {remaining:?}"
        );
    }

    #[test]
    fn disable_comment_stays_out_of_a_long_string() {
        let text = "\
local s = [[
x]] local y = nil
";
        let config = config_enabling("redundant_nil_init");
        let edited = disabled(text, &config, 0);
        assert!(
            edited.contains(
                "\
[[
x]]"
            ),
            "the comment must not rewrite the string's contents: {edited:?}"
        );
    }

    #[test]
    fn disable_comment_does_not_silence_the_next_statement() {
        let text = "\
local t = {
  first_missing,
}
local u = second_missing
";
        let config = config_enabling("undefined_variable");
        let edited = disabled(text, &config, 0);
        let (_, remaining) = linted(&edited, &config);
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
        let (reparsed, remaining) = linted(&edited, &config);
        assert!(reparsed.parse().errors.is_empty(), "{:?}", reparsed.parse().errors);
        assert_eq!(remaining.len(), 1, "the later statement stays flagged: {remaining:?}");
    }

    fn lint_with_fix(edits: Vec<LinterTextEdit>) -> Message {
        Message {
            diagnostic: LuckDiagnostic::lint(
                "test_rule",
                Severity::Warning,
                Span::new(0, 0),
                "bad",
            ),
            fix: Some(Fix { kind: FixKind::Fix, message: "fix".into(), edits: edits.into() }),
        }
    }

    fn linter_edit(start: u32, end: u32) -> LinterTextEdit {
        LinterTextEdit { span: Span::new(start, end), replacement: b"x".as_slice().into() }
    }

    /// An unused variable, whose rename is a suggestion, beside a redundant
    /// `nil`, whose deletion is a fix.
    fn suggestion_and_fix() -> (DocumentState, Vec<Message>) {
        let mut config = LintConfig::default();
        config.rules.set_level("redundant_nil_init", RuleLevel::Warn);
        linted("local unused = nil\n", &config)
    }

    #[test]
    fn fix_all_leaves_suggestions_out() {
        let (document, lints) = suggestion_and_fix();
        let edits = all_fix_edits(&document, &lints);
        assert_eq!(applied(&document, edits), "local unused\n");
    }

    #[test]
    fn a_suggestion_is_a_quick_fix_the_client_never_prefers() {
        let (document, lints) = suggestion_and_fix();
        let diags = diagnostics::to_lsp(&uri(), &document, &lints);
        let range = Range::new(Position::new(0, 0), Position::new(0, 18));
        let quick_fixes = Some(vec![CodeActionKind::QUICKFIX]);
        let actions = code_action(&document, &params(range, diags, quick_fixes), &lints);
        let fixes: Vec<(&str, Option<bool>)> = actions
            .iter()
            .map(only_action)
            .filter(|action| !action.title.starts_with("Disable"))
            .map(|action| (action.title.as_str(), action.is_preferred))
            .collect();
        assert_eq!(
            fixes,
            [
                ("drop redundant `= nil` initializer", Some(true)),
                ("prefix `unused` with `_`", Some(false)),
            ]
        );
    }

    #[test]
    fn a_fix_that_overlaps_a_later_one_drops_whole() {
        let (document, _) = linted("0123456789\n", &LintConfig::default());
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
