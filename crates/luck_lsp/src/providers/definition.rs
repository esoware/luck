//! Go-to-definition. Local symbols jump to their declaration through the
//! scope tree; `require("path")` strings jump to the resolved module file.
//! Globals have no definition site in a single-file view and return `None`.

use tower_lsp::lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, Location, Position, Url};

use crate::backend::DocumentState;
use crate::providers::cursor::symbol_at;
use crate::providers::document_link;

#[must_use]
pub fn goto_definition(
    doc: &DocumentState,
    uri: &Url,
    params: &GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let position = params.text_document_position_params.position;

    if let Some(location) = require_target_at(doc, uri, position) {
        return Some(GotoDefinitionResponse::Scalar(location));
    }

    let offset = doc.line_index.offset(&doc.text, position);
    let symbol_id = symbol_at(&doc.semantic.scope_tree, offset)?;
    let symbol = &doc.semantic.scope_tree.symbols[symbol_id.index()];
    let range = doc.line_index.range(
        &doc.text,
        symbol.definition_span.start,
        symbol.definition_span.end,
    );
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range,
    }))
}

/// If the cursor sits inside a `require("...")` string that resolves to a
/// file, jump to the top of that file.
fn require_target_at(doc: &DocumentState, uri: &Url, position: Position) -> Option<Location> {
    document_link::document_links(doc, uri)
        .into_iter()
        .find(|link| {
            position >= link.range.start && position <= link.range.end && link.target.is_some()
        })
        .and_then(|link| {
            Some(Location {
                uri: link.target?,
                range: tower_lsp::lsp_types::Range::default(),
            })
        })
}
