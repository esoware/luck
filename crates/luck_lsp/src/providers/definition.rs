//! Local symbols jump to their declaration through the scope tree;
//! `require("path")` strings jump to the resolved module file. Globals have
//! no definition site in a single-file view and return `None`.

use tower_lsp::lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, Location, Range};

use crate::document::DocumentState;
use crate::providers::cursor::local_at;
use crate::providers::document_link;
use crate::settings::ProjectSettings;

pub(crate) fn goto(
    document: &DocumentState,
    settings: &ProjectSettings,
    params: &GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    // A `require("...")` string under the cursor that resolves to a file
    // jumps to the top of that file.
    if let Some(module_uri) = document_link::target_at(document, uri, settings, position) {
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: module_uri,
            range: Range::default(),
        }));
    }

    let tree = &document.semantic.scope_tree;
    let (symbol_id, _) = local_at(tree, document.offset(position))?;
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range: document.range(tree.symbol(symbol_id).definition_span),
    }))
}
