//! A `require` string goes to its module's file, a member of a module's
//! table to where that module gives it its value, and a local to its
//! declaration. A global has no one place that defines it.

use ls_types::{GotoDefinitionResponse, Location, Position, Range, Uri};

use crate::document::DocumentState;
use crate::providers::cursor::local_at;
use crate::providers::document_link;
use crate::workspace::{Source, WorkspaceView};

pub(crate) fn goto(
    workspace: &WorkspaceView<'_>,
    uri: &Uri,
    document: &DocumentState,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    if let Some(module_uri) = document_link::target_at(document, position) {
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: module_uri,
            range: Range::default(),
        }));
    }
    if let Some((member, module)) =
        Source::of_document(uri, document).and_then(|source| source.member_at(position))
    {
        let mut definitions = workspace
            .with_source(&module, |target| {
                target
                    .record
                    .members()
                    .iter()
                    .filter(|defined| defined.is_definition && defined.name == member.name)
                    .map(|defined| Location { uri: target.uri.clone(), range: defined.range })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        return match definitions.len() {
            0 => None,
            1 => definitions.pop().map(GotoDefinitionResponse::Scalar),
            _ => Some(GotoDefinitionResponse::Array(definitions)),
        };
    }
    let tree = document.tree();
    let (symbol_id, _) = local_at(document, document.offset(position))?;
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range: document.range(tree.symbol(symbol_id).definition_span),
    }))
}
