//! Matching is by span, so shadowed names never bleed into each other, and
//! a local can't be referenced from another file, so one document is the
//! whole answer.

use luck_semantic::SymbolKind;
use luck_token::Span;
use tower_lsp_server::ls_types::{Location, ReferenceParams};

use crate::document::DocumentState;
use crate::providers::cursor::local_at;

pub(crate) fn references(document: &DocumentState, params: &ReferenceParams) -> Vec<Location> {
    let uri = &params.text_document_position.text_document.uri;
    let offset = document.offset(params.text_document_position.position);
    let tree = &document.semantic.scope_tree;
    let Some((symbol_id, _)) = local_at(tree, offset) else {
        return Vec::new();
    };
    let symbol = tree.symbol(symbol_id);

    let mut spans: Vec<Span> = symbol
        .reference_ids
        .iter()
        .map(|&reference_id| tree.reference(reference_id).span)
        .collect();
    // The implicit `self`'s declaration has no token, only an empty span.
    if params.context.include_declaration && symbol.kind != SymbolKind::ImplicitSelf {
        spans.push(symbol.definition_span);
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();

    spans
        .into_iter()
        .map(|span| Location { uri: uri.clone(), range: document.range(span) })
        .collect()
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;
    use tower_lsp_server::ls_types::{
        PartialResultParams, Position, ReferenceContext, TextDocumentIdentifier,
        TextDocumentPositionParams, WorkDoneProgressParams,
    };

    use super::*;

    #[test]
    fn the_implicit_self_declaration_is_no_location() {
        let document = DocumentState::new(
            "local t = {}\nfunction t:m()\n  return self\nend\n".to_owned(),
            1,
            LuaTarget::Lua54,
            FxHashSet::default(),
        );
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: "file:///main.lua".parse().expect("valid URI"),
                },
                position: Position::new(2, 10),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext { include_declaration: true },
        };
        let locations = references(&document, &params);
        assert_eq!(locations.len(), 1, "only the read of `self`: {locations:?}");
        assert_eq!(locations[0].range.start, Position::new(2, 9), "{locations:?}");
    }
}
