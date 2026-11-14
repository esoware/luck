pub(crate) mod code_action;
pub(crate) mod completion;
mod cursor;
pub(crate) mod definition;
pub(crate) mod diagnostics;
pub(crate) mod document_highlight;
pub(crate) mod document_link;
pub(crate) mod document_symbol;
pub(crate) mod folding_range;
pub(crate) mod hover;
pub(crate) mod references;
pub(crate) mod rename;
pub(crate) mod selection_range;
pub(crate) mod semantic_tokens;
pub(crate) mod signature_help;
mod stdlib_render;
pub(crate) mod syntax_tree;
pub(crate) mod workspace_symbol;

/// Every request fits the smallest stack, however the input nests.
///
/// The server analyzes what's typed as it's typed, broken or not, and every
/// request walks that tree, so each runs on input nested to the parser's cap,
/// past it, broken at every level, or holding long runs and lists.
#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use luck_core::LuaTarget;
    use luck_formatter::FormatOptions;
    use luck_linter::LintConfig;
    use luck_testgen::{
        broken_sources, deep_and_long_sources, on_the_smallest_stack, sources_past_the_nesting_cap,
    };
    use luck_token::LuaVersion;
    use rustc_hash::FxHashSet;
    use tower_lsp_server::ls_types::{
        CodeActionContext, CodeActionParams, GotoDefinitionParams, PartialResultParams,
        ReferenceContext, ReferenceParams, RenameParams, TextDocumentIdentifier,
        TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    };

    use super::*;
    use crate::document::DocumentState;
    use crate::settings::ProjectSettings;

    fn serve_every_request(source: &str, version: LuaVersion) {
        let target = if version.is_luau() { LuaTarget::LuauRoblox } else { LuaTarget::Lua54 };
        let uri: Uri = "file:///main.lua".parse().expect("a valid URI");
        let document = DocumentState::new(source.to_owned(), 1, target, FxHashSet::default());
        let settings = ProjectSettings::default();
        let lints = document.lint(&LintConfig::default());
        let diagnostics = diagnostics::to_lsp(&document, &lints);
        let full_range = document.full_range();
        let text_document = TextDocumentIdentifier { uri: uri.clone() };
        let _actions = code_action::code_action(
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
            let _hover = hover::hover(&document, position);
            let _completion = completion::completion(&document, position);
            let _signature = signature_help::signature_help(&document, position);
            let _highlights = document_highlight::document_highlights(&document, position);
            let _prepared = rename::prepare(&document, position);
            let _renamed = rename::rename(
                &document,
                &RenameParams {
                    text_document_position: at.clone(),
                    new_name: "renamed".to_owned(),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                },
            );
            let _references = references::references(
                &document,
                &ReferenceParams {
                    text_document_position: at.clone(),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                    context: ReferenceContext { include_declaration: true },
                },
            );
            let _definition = definition::goto(
                &document,
                &settings,
                &GotoDefinitionParams {
                    text_document_position_params: at,
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                },
            );
        }
        let _selections = selection_range::selection_ranges(&document, &positions);
        let _folds = folding_range::folding_ranges(&document);
        let _symbols = document_symbol::document_symbols(&document);
        let _tokens = semantic_tokens::semantic_tokens(&document);
        let _ranged_tokens = semantic_tokens::in_range(&document, full_range);
        let _links = document_link::document_links(&document, &uri, &settings);
        let _tree = syntax_tree::syntax_tree(&document);
        let documents = [(uri, Arc::new(document))];
        let _workspace_symbols = workspace_symbol::workspace_symbols(&documents, "a");
        let _range_formatted = luck_formatter::format_range(
            source,
            version,
            &FormatOptions::default(),
            0..source.len(),
        );
    }

    #[test]
    fn every_request_on_deep_and_long_input_fits_the_smallest_stack() {
        on_the_smallest_stack(deep_and_long_sources(), serve_every_request);
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
