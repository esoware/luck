//! Find references.
//!
//! Resolves the local at the cursor through the scope tree and returns every
//! reference to it. Matching is by span, so shadowed names never bleed into
//! each other, and a local can't be referenced from another file, so one
//! document is the whole answer.

use luck_token::Span;
use tower_lsp::lsp_types::{Location, ReferenceParams};

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
    if params.context.include_declaration {
        spans.push(symbol.definition_span);
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();

    spans
        .into_iter()
        .map(|span| Location { uri: uri.clone(), range: document.range(span) })
        .collect()
}
