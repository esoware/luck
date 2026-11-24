//! A local's declaration and references, or every reference to a global of
//! the same name, each a write or a read.

use ls_types::{DocumentHighlight, DocumentHighlightKind, Position};
use luck_token::Span;

use crate::document::DocumentState;
use crate::providers::cursor::name_at;

pub(crate) fn document_highlights(
    document: &DocumentState,
    position: Position,
) -> Vec<DocumentHighlight> {
    let Some((_, name)) = name_at(document, document.offset(position)) else {
        return Vec::new();
    };
    let tree = document.scoping();
    let at = tree.reference_of(name).map(|id| tree.reference(id));
    let mut spans: Vec<(Span, bool)> = Vec::new();
    match (tree.symbol_of(name).or_else(|| at?.resolved), at) {
        (Some(symbol_id), _) => {
            let symbol = tree.symbol(symbol_id);
            // The implicit `self` and Lua 5.1 `arg` have no name to mark.
            if !symbol.kind.is_implicit() {
                spans.push((symbol.definition_span, true));
            }
            for &reference_id in tree.resolved_references(symbol_id) {
                let reference = tree.reference(reference_id);
                spans.push((reference.span, reference.kind.is_write()));
            }
        }
        // A free name under a local `_ENV` is a field of that table, so it's
        // the same one only under the same `_ENV`.
        (None, Some(global)) => {
            for reference in tree.references() {
                if reference.resolved.is_none()
                    && reference.name == global.name
                    && reference.env == global.env
                {
                    spans.push((reference.span, reference.kind.is_write()));
                }
            }
        }
        (None, None) => {}
    }
    // References come in evaluation order, which is mostly source order.
    spans.sort_unstable_by_key(|&(span, _)| span.start);
    let mut positions = document.positions();
    spans
        .into_iter()
        .map(|(span, is_write)| DocumentHighlight {
            range: positions.range(span),
            kind: Some(if is_write {
                DocumentHighlightKind::WRITE
            } else {
                DocumentHighlightKind::READ
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::document;

    /// The highlights at `utf16_column` on the first line, as the columns
    /// they span and whether each is a write.
    fn highlighted(text: &str, target: LuaTarget, utf16_column: u32) -> Vec<(u32, u32, bool)> {
        let document = document(text, target);
        document_highlights(&document, Position::new(0, utf16_column))
            .into_iter()
            .map(|highlight| {
                let range = highlight.range;
                assert_eq!(range.start.line, 0, "{highlight:?}");
                let is_write = highlight.kind == Some(DocumentHighlightKind::WRITE);
                (range.start.character, range.end.character, is_write)
            })
            .collect()
    }

    #[test]
    fn highlight_marks_compound_assignment_as_a_write() {
        let text = "\
local n = 1
n += 1
";
        let document = document(text, LuaTarget::Luau);
        let highlights = document_highlights(&document, Position::new(1, 0));
        let kinds: Vec<_> = highlights.iter().map(|highlight| highlight.kind).collect();
        assert_eq!(
            kinds,
            [Some(DocumentHighlightKind::WRITE), Some(DocumentHighlightKind::WRITE)],
            "the declaration and `n += 1` both write `n`: {highlights:?}"
        );
    }

    #[test]
    fn highlight_keeps_to_the_local_a_name_means() {
        // `x` is declared at 6, shadowed at 21 and read at 29, and read at 38.
        let text = "local x = 1 do local x = 2 f(x) end f(x)";
        let outer = [(6, 7, true), (38, 39, false)];
        assert_eq!(highlighted(text, LuaTarget::Lua54, 6), outer);
        assert_eq!(highlighted(text, LuaTarget::Lua54, 38), outer);
        assert_eq!(highlighted(text, LuaTarget::Lua54, 30), [(21, 22, true), (29, 30, false)]);
    }

    #[test]
    fn highlight_marks_every_use_of_a_global_and_no_field() {
        let text = "g = 1 print(g, t.g)";
        assert_eq!(highlighted(text, LuaTarget::Lua54, 12), [(0, 1, true), (12, 13, false)]);
        assert!(highlighted(text, LuaTarget::Lua54, 17).is_empty(), "a field is no name to match");
    }
}
