//! A `require` string goes to its module's file, a member of a module's
//! table to where that module gives it its value, and a local to its
//! declaration. A global, a type, or a member a definition file declares goes
//! to that declaration, which for Luau's and Roblox's is in a file the server
//! serves itself.

use ls_types::{GotoDefinitionResponse, Location, Position, Range, Uri};

use crate::document::DocumentState;
use crate::providers::cursor::local_at;
use crate::providers::document_link;
use crate::type_check::Types;
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
    let tree = document.scoping();
    let (symbol_id, _) = local_at(document, document.offset(position))?;
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range: document.range(tree.symbol(symbol_id).definition_span),
    }))
}

/// Where a definition file declares what the name that starts at
/// `name_start` refers to, which
/// [`name_start`](crate::providers::cursor::name_start) finds.
pub(crate) fn declared(name_start: u32, types: &mut Types<'_>) -> Option<GotoDefinitionResponse> {
    let description = types.describe(name_start)?;
    types.location(&description.declaration).map(GotoDefinitionResponse::Scalar)
}
