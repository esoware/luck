//! Document highlights. For a name under the cursor, return every other
//! occurrence of the same name in the document.

use luck_semantic::scope::ReferenceKind;
use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams};

use crate::backend::DocumentState;
use crate::providers::cursor::find_target_at;

#[must_use]
pub fn document_highlight(
    doc: &DocumentState,
    params: &DocumentHighlightParams,
) -> Vec<DocumentHighlight> {
    let position = params.text_document_position_params.position;
    let offset = doc.line_index.offset(&doc.text, position);
    let Some(target) = find_target_at(&doc.parsed.block, offset) else {
        return Vec::new();
    };
    let name = match &target {
        crate::providers::cursor::CursorTarget::Identifier { name, .. } => name.as_str(),
        crate::providers::cursor::CursorTarget::DottedPath { segments, .. } => {
            segments.last().map(String::as_str).unwrap_or("")
        }
        crate::providers::cursor::CursorTarget::Call { path, .. } => {
            path.last().map(String::as_str).unwrap_or("")
        }
    };
    if name.is_empty() {
        return Vec::new();
    }

    // Semantic analysis is computed once per edit and cached on the
    // document - re-running it here made every cursor-idle highlight
    // re-walk the whole file.
    doc.semantic
        .scope_tree
        .references
        .iter()
        .filter(|r| r.name == name)
        .map(|r| DocumentHighlight {
            range: doc.line_index.range(&doc.text, r.span.start, r.span.end),
            kind: Some(match r.kind {
                ReferenceKind::Write => DocumentHighlightKind::WRITE,
                ReferenceKind::Read | ReferenceKind::ReadWrite => DocumentHighlightKind::READ,
            }),
        })
        .collect()
}
