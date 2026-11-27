pub(crate) mod code_action;
pub(crate) mod completion;
pub(crate) mod cursor;
pub(crate) mod definition;
pub(crate) mod diagnostics;
pub(crate) mod document_highlight;
pub(crate) mod document_link;
pub(crate) mod document_symbol;
pub(crate) mod folding_range;
pub(crate) mod formatting;
pub(crate) mod hover;
pub(crate) mod references;
pub(crate) mod rename;
pub(crate) mod selection_range;
pub(crate) mod semantic_tokens;
pub(crate) mod signature_help;
pub(crate) mod syntax_tree;
pub(crate) mod workspace_symbol;

/// Every request fits the smallest stack, however the input nests.
///
/// The server analyzes what's typed as it's typed, broken or not, and every
/// request walks that tree, so each runs on input nested to the parser's cap,
/// past it, broken at every level, or holding long runs and lists. The
/// requests Luau's type checker answers run on all but the long runs and
/// lists, as `luck check`'s own stack test leaves them out, since Luau takes
/// tens of seconds to check a function of 20,000 parameters.
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ls_types::{
        CodeActionContext, CodeActionParams, PartialResultParams, ReferenceContext,
        ReferenceParams, RenameParams, TextDocumentIdentifier, TextDocumentPositionParams,
        WorkDoneProgressParams,
    };
    use luck_config::Project;
    use luck_resolver::FileSystemOs;
    use luck_testgen::{
        broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_at_the_nesting_cap,
        sources_past_the_nesting_cap,
    };
    use luck_type_checker::CancellationToken;

    use super::*;
    use crate::fixtures::{document_at, session, uri};
    use crate::providers::cursor::name_start;
    use crate::type_check::with_types;
    use crate::workspace::WorkspaceView;

    fn serve_every_request(source: &str) {
        serve_every_request_but_types(source);
        serve_every_typed_request(source);
    }

    fn serve_every_typed_request(source: &str) {
        let uri = uri();
        let project = Project::defaults(PathBuf::from("."), FileSystemOs);
        let document = document_at(source, project, Path::new("main.luau"));
        let session = session();
        let positions: Vec<_> = [0, source.len().div_ceil(2), source.len()]
            .into_iter()
            .map(|index| document.position(u32::try_from(index).expect("a small source")))
            .collect();
        let checked =
            with_types(&session, &uri, &document, &[], &CancellationToken::new(), |types| {
                for &position in &positions {
                    if let Some(start) = name_start(&document, position) {
                        let _ = hover::hover(&document, start, types);
                        let _ = definition::declared(start, types);
                    }
                    let _ = completion::completion(&document, position, types);
                    if let Some(call) = signature_help::call_at(&document, position) {
                        let _ = signature_help::signature_help(&document, call, types);
                    }
                }
            });
        assert!(checked.is_some(), "nothing cancels the check");
    }

    fn serve_every_request_but_types(source: &str) {
        let uri = uri();
        let project = Project::defaults(PathBuf::from("."), FileSystemOs);
        let document = document_at(source, project, Path::new("main.luau"));
        let session = session();
        let workspace = WorkspaceView::new(&session, &[], &|| false);
        let lints = document.lint(&|| false).expect("nothing cancels it");
        let diagnostics = diagnostics::to_lsp(&uri, &document, &lints);
        let full_range = document.full_range();
        let text_document = TextDocumentIdentifier { uri: uri.clone() };
        let _ = code_action::code_action(
            &document,
            &CodeActionParams {
                text_document: text_document.clone(),
                range: full_range,
                context: CodeActionContext { diagnostics, only: None, trigger_kind: None },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
            &lints,
        );
        let positions: Vec<_> = [0, source.len().div_ceil(2), source.len()]
            .into_iter()
            .map(|index| document.position(u32::try_from(index).expect("a small source")))
            .collect();
        for &position in &positions {
            let at = TextDocumentPositionParams { text_document: text_document.clone(), position };
            let _ = document_highlight::document_highlights(&document, position);
            let _ = rename::prepare(&workspace, &uri, &document, position);
            let _ = rename::rename(
                &workspace,
                &uri,
                &document,
                &RenameParams {
                    text_document_position: at.clone(),
                    new_name: "renamed".to_owned(),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                },
            );
            let _ = references::references(
                &workspace,
                &uri,
                &document,
                &ReferenceParams {
                    text_document_position: at,
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                    context: ReferenceContext { include_declaration: true },
                },
            );
            let _ = definition::goto(&workspace, &uri, &document, position);
        }
        let _ = selection_range::selection_ranges(&document, &positions);
        let _ = folding_range::folding_ranges(&document);
        let _ = document_symbol::document_symbols(&document);
        let _ = semantic_tokens::semantic_tokens(&document);
        let _ = semantic_tokens::in_range(&document, full_range);
        let _ = document_link::document_links(&document);
        let _ = formatting::formatting(&session, &document, None);
        let _ = formatting::formatting(&session, &document, Some(full_range));
        let _ = workspace_symbol::workspace_symbols([(&uri, &document)], "a");
        let _ = code_action::fix_all([(&uri, &document, lints.as_slice())]);
    }

    #[test]
    fn every_request_on_deep_and_long_input_fits_the_smallest_stack() {
        on_the_smallest_stack(deep_and_long_sources(), serve_every_request_but_types);
        on_the_smallest_stack(sources_at_the_nesting_cap(), serve_every_typed_request);
    }

    #[test]
    fn every_request_on_input_past_the_cap_fits_the_smallest_stack() {
        on_the_smallest_stack(sources_past_the_nesting_cap(), serve_every_request);
    }

    #[test]
    fn every_request_on_broken_input_fits_the_smallest_stack() {
        on_the_smallest_stack(broken_sources(), serve_every_request);
    }
}
