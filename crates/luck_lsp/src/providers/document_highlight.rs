//! Document highlights: every read and write of the name under the cursor.
//! Matching is by name across the whole file, and declarations aren't
//! included.

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
            kind: Some(if reference.kind.is_write() {
                DocumentHighlightKind::WRITE
            } else {
                DocumentHighlightKind::READ
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;

    use super::*;

    #[test]
    fn highlight_marks_compound_assignment_as_a_write() {
        let text = "local n = 1\nn += 1\n";
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Luau, FxHashSet::default());
        let highlights = document_highlights(&document, Position::new(1, 0));
        assert_eq!(
            highlights.first().and_then(|highlight| highlight.kind),
            Some(DocumentHighlightKind::WRITE),
            "`n += 1` writes `n`: {highlights:?}"
        );
    }
}
