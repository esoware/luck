//! Document highlights: every read and write of the name under the cursor.
//! Matching is by name across the whole file, and declarations aren't
//! included.

use luck_semantic::ReferenceKind;
use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, Position};

use crate::document::DocumentState;
use crate::providers::cursor::find_node_at;

pub(crate) fn document_highlights(
    document: &DocumentState,
    position: Position,
) -> Vec<DocumentHighlight> {
    let offset = document.offset(position);
    let Some(node) = find_node_at(&document.parsed.block, offset) else {
        return Vec::new();
    };
    let Some(&name) = node.path().last() else {
        return Vec::new();
    };
    document
        .semantic
        .scope_tree
        .references
        .iter()
        .filter(|reference| reference.name == name)
        .map(|reference| DocumentHighlight {
            range: document.range(reference.span),
            kind: Some(match reference.kind {
                ReferenceKind::Write => DocumentHighlightKind::WRITE,
                ReferenceKind::Read | ReferenceKind::ReadWrite => DocumentHighlightKind::READ,
            }),
        })
        .collect()
}
