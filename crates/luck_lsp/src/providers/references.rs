//! A member's references span the workspace, and a local's its document.
//!
//! A member of a module's table is found in every file of the workspace that
//! names it, through the module's own table or a require of the module. A
//! name finds the local it declares or refers to by its symbol, so shadowed
//! names never bleed into each other, and a local can't be referenced from
//! another file, so one document is the whole answer.

use ls_types::{Location, ReferenceParams, Uri};
use luck_token::Span;

use crate::document::DocumentState;
use crate::providers::cursor::local_at;
use crate::workspace::{Source, WorkspaceView};

/// `None` once the request is cancelled.
pub(crate) fn references(
    workspace: &WorkspaceView<'_>,
    uri: &Uri,
    document: &DocumentState,
    params: &ReferenceParams,
) -> Option<Vec<Location>> {
    let position = params.text_document_position.position;
    let include_declaration = params.context.include_declaration;
    if let Some((member, module)) =
        Source::of_document(uri, document).and_then(|source| source.member_at(position))
    {
        let mut locations = Vec::new();
        workspace.for_each_source(|source| {
            let found = source.record.members().iter().filter(|named| {
                named.name == member.name
                    && (include_declaration || !named.is_definition)
                    && source.is_table_of(named.table, &module)
            });
            locations.extend(
                found.map(|named| Location { uri: source.uri.clone(), range: named.range }),
            );
        })?;
        return Some(locations);
    }
    let tree = document.scoping();
    let Some((symbol_id, _)) = local_at(document, document.offset(position)) else {
        return Some(Vec::new());
    };
    let symbol = tree.symbol(symbol_id);

    let mut spans: Vec<Span> = tree
        .resolved_references(symbol_id)
        .iter()
        .map(|&reference_id| tree.reference(reference_id).span)
        .collect();
    // The implicit `self`'s declaration has no token, only an empty span.
    if include_declaration && !symbol.kind.is_implicit() {
        spans.push(symbol.definition_span);
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();

    let mut positions = document.positions();
    Some(
        spans
            .into_iter()
            .map(|span| Location { uri: uri.clone(), range: positions.range(span) })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use ls_types::{
        PartialResultParams, Position, ReferenceContext, TextDocumentIdentifier,
        TextDocumentPositionParams, WorkDoneProgressParams,
    };
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::{document, session, uri};

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
        let uri = uri();
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position::new(2, 10),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext { include_declaration: true },
        };
        let session = session();
        let workspace = WorkspaceView::new(&session, &[], &|| false);
        let locations =
            references(&workspace, &uri, &document, &params).expect("nothing cancels it");
        assert_eq!(locations.len(), 1, "only the read of `self`: {locations:?}");
        assert_eq!(locations[0].range.start, Position::new(2, 9), "{locations:?}");
    }
}
