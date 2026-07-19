//! Find references. Resolves the symbol at the cursor through the scope
//! tree and returns every reference span - span-exact, so shadowed names
//! never bleed into each other. Locals cannot cross files in Lua, so a
//! single-document answer is complete for them.

use tower_lsp::lsp_types::{Location, ReferenceParams, Url};

use crate::backend::DocumentState;
use crate::providers::cursor::symbol_at;

#[must_use]
pub fn references(doc: &DocumentState, uri: &Url, params: &ReferenceParams) -> Vec<Location> {
    let position = params.text_document_position.position;
    let offset = doc.line_index.offset(&doc.text, position);
    let tree = &doc.semantic.scope_tree;
    let Some(symbol_id) = symbol_at(tree, offset) else {
        return Vec::new();
    };
    let symbol = &tree.symbols[symbol_id.index()];

    let mut spans: Vec<luck_token::Span> = symbol
        .reference_ids
        .iter()
        .map(|&reference_id| tree.references[reference_id.index()].span)
        .collect();
    if params.context.include_declaration {
        spans.push(symbol.definition_span);
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();

    spans
        .into_iter()
        .map(|span| Location {
            uri: uri.clone(),
            range: doc.line_index.range(&doc.text, span.start, span.end),
        })
        .collect()
}
