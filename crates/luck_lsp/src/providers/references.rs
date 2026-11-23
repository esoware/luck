//! Matching is by span, so shadowed names never bleed into each other, and
//! a local can't be referenced from another file, so one document is the
//! whole answer.

use ls_types::{Location, ReferenceParams};
use luck_token::Span;

use crate::document::DocumentState;
use crate::providers::cursor::local_at;

pub(crate) fn references(document: &DocumentState, params: &ReferenceParams) -> Vec<Location> {
    let uri = &params.text_document_position.text_document.uri;
    let offset = document.offset(params.text_document_position.position);
    let tree = document.tree();
    let Some((symbol_id, _)) = local_at(tree, offset) else {
        return Vec::new();
    };
    let symbol = tree.symbol(symbol_id);

    let mut spans: Vec<Span> = tree
        .resolved_references(symbol_id)
        .iter()
        .map(|&reference_id| tree.reference(reference_id).span)
        .collect();
    // The implicit `self`'s declaration has no token, only an empty span.
    if params.context.include_declaration && !symbol.kind.is_implicit() {
        spans.push(symbol.definition_span);
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();

    let mut positions = document.positions();
    spans
        .into_iter()
        .map(|span| Location { uri: uri.clone(), range: positions.range(span) })
        .collect()
}

#[cfg(test)]
mod tests {
    use ls_types::{
        PartialResultParams, Position, ReferenceContext, TextDocumentIdentifier,
        TextDocumentPositionParams, WorkDoneProgressParams,
    };
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::document;

    #[test]
    fn the_implicit_self_declaration_is_no_location() {
        let document = document(
            "\
local t = {}
function t:m()
  return self
end
",
            LuaTarget::Lua54,
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
