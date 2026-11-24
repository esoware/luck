use core::iter;

use ls_types::{Position, SelectionRange};
use luck_ast::{AstKind, GetSpan as _};
use luck_token::{Associativity, Span, TokenKind};

use crate::document::{DocumentState, span_text};

/// How many of the innermost enclosing spans reach the editor. The `parent`
/// chain's drop and serialization recurse, so a long run of operators or
/// suffixes must not become an equally long list.
const MAX_SELECTION_DEPTH: usize = 256;

pub(crate) fn selection_ranges(
    document: &DocumentState,
    positions: &[Position],
) -> Vec<SelectionRange> {
    positions.iter().map(|&position| selection_for(document, position)).collect()
}

/// The word at the cursor, then each node it's in, innermost first.
///
/// The word may be a keyword or a literal no node spells. Each span is
/// followed by the parentheses the source wrapped it in, and a run of
/// operators or suffixes steps out through each step as if it nested.
fn selection_for(document: &DocumentState, position: Position) -> SelectionRange {
    let offset = document.offset(position);
    let groupings = &document.parse().groupings;
    let extent = |span: Span| groupings.around(span).unwrap_or(span);
    // Innermost first, each holding the one before it.
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |span: Span| {
        for selected in iter::once(span).chain(groupings.around(span)) {
            let holds_last = spans.last().is_none_or(|&last| {
                selected != last && selected.start <= last.start && last.end <= selected.end
            });
            if holds_last && selected.start <= offset && offset <= selected.end {
                spans.push(selected);
            }
        }
    };
    // Of the two tokens that can meet at the cursor, the later.
    let tokens = document.tokens();
    let touching = tokens.get(tokens.partition_point(|token| token.span.end < offset)..);
    if let Some(word) = touching.unwrap_or_default().iter().take(2).rfind(|token| {
        token.span.start <= offset
            && token.kind != TokenKind::Eof
            && TokenKind::is_identifier_shaped(span_text(document.text(), token.span))
    }) {
        push(word.span);
    }
    let nodes = document.nodes();
    let innermost = nodes.node_at(offset);
    let ids = innermost.into_iter().flat_map(|id| iter::once(id).chain(nodes.ancestor_ids(id)));
    for kind in ids.map(|id| nodes.kind(id)) {
        if let AstKind::BinaryOp(run) = kind {
            let operands = || run.links.iter().map(|link| &link.operand);
            let inner_count = run.links.len().saturating_sub(1);
            if run.precedence().1 == Associativity::Left {
                let start = extent(run.first.span()).start;
                for operand in operands().take(inner_count) {
                    push(Span::new(start, extent(operand.span()).end));
                }
            } else {
                let end = extent(run.links.last().operand.span()).end;
                for operand in operands().take(inner_count).rev() {
                    push(Span::new(extent(operand.span()).start, end));
                }
            }
        }
        if let AstKind::SuffixedExpression(chain) = kind {
            // Each head is the chain it was before the next suffix joined it,
            // in the parentheses it was in.
            let mut start = extent(chain.prefix.span()).start;
            let inner_count = chain.suffixes.len().saturating_sub(1);
            for suffix in chain.suffixes.iter().take(inner_count) {
                let head = Span::new(start, suffix.span().end);
                push(head);
                start = extent(head).start;
            }
        }
        push(kind.span());
    }
    // The returned link is the narrowest span and each `parent` the next one
    // out, so the chain folds from the outside in.
    let mut chain: Option<SelectionRange> = None;
    for &span in spans.iter().take(MAX_SELECTION_DEPTH).rev() {
        chain = Some(SelectionRange { range: document.range(span), parent: chain.map(Box::new) });
    }
    chain.unwrap_or_else(|| SelectionRange {
        range: document.range(Span::new(offset, offset)),
        parent: None,
    })
}

#[cfg(test)]
mod tests {
    use ls_types::Range;
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::document;

    /// The selection at `offset` on the first line, innermost first.
    fn ranges_at(source: &str, offset: u32) -> Vec<Range> {
        let document = document(source, LuaTarget::Lua54);
        assert!(document.parse().errors.is_empty(), "{:?}", document.parse().errors);
        let selection = selection_for(&document, Position::new(0, offset));
        let mut ranges = vec![selection.range];
        let mut current = &selection;
        while let Some(parent) = current.parent.as_deref() {
            ranges.push(parent.range);
            current = parent;
        }
        ranges
    }

    fn columns(ranges: &[Range]) -> Vec<(u32, u32)> {
        ranges.iter().map(|range| (range.start.character, range.end.character)).collect()
    }

    #[test]
    fn a_selection_chain_at_a_statement_boundary_is_nested() {
        // Offset 3 ends `f()` and starts `g()`. Either direction of nesting
        // passes, so this is independent of the chain's order.
        let ranges = ranges_at("f()g()\n", 3);
        let contains =
            |outer: Range, inner: Range| outer.start <= inner.start && inner.end <= outer.end;
        for pair in ranges.windows(2) {
            if let [first, second] = *pair {
                assert!(
                    contains(first, second) || contains(second, first),
                    "sibling statements can't share one selection chain: {ranges:?}"
                );
            }
        }
    }

    #[test]
    fn a_run_of_operators_steps_out_through_each_pair() {
        // `x = a + b + c`, at `b`: `b`, `a + b`, the run, the statement.
        assert_eq!(columns(&ranges_at("x = a + b + c\n", 8)), [(8, 9), (4, 9), (4, 13), (0, 13)]);
        // `x = a .. b .. c` groups from the right: `b`, `b .. c`, the run.
        assert_eq!(
            columns(&ranges_at("x = a .. b .. c\n", 9)),
            [(9, 10), (9, 15), (4, 15), (0, 15)]
        );
    }

    #[test]
    fn a_run_of_suffixes_steps_out_through_each_head() {
        // `x = a.b:c(d).e`, at `a`: `a`, `a.b`, `a.b:c(d)`, the chain.
        assert_eq!(
            columns(&ranges_at("x = a.b:c(d).e\n", 4)),
            [(4, 5), (4, 7), (4, 12), (4, 14), (0, 14)]
        );
    }

    #[test]
    fn parentheses_step_out_around_what_they_hold() {
        // `x = (a + b) * c`, at `a`: `a`, `a + b`, `(a + b)`, the product.
        assert_eq!(
            columns(&ranges_at("x = (a + b) * c\n", 5)),
            [(5, 6), (5, 10), (4, 11), (4, 15), (0, 15)]
        );
        // `x = ((a).b).c`, at `a`: `a`, `(a)`, `(a).b`, `((a).b)`, the chain.
        assert_eq!(
            columns(&ranges_at("x = ((a).b).c\n", 6)),
            [(6, 7), (5, 8), (5, 10), (4, 11), (4, 13), (0, 13)]
        );
    }

    #[test]
    fn a_word_no_node_spells_is_selected_first() {
        // `nil` and `break` are no nodes, but the word is what's selected
        // first, and then what holds it.
        assert_eq!(columns(&ranges_at("f(nil)\n", 3)), [(2, 5), (0, 6)]);
        assert_eq!(
            columns(&ranges_at("while x do break end\n", 12)),
            [(11, 16), (0, 20)],
            "a lone statement is its block"
        );
    }

    #[test]
    fn a_selection_range_parent_contains_its_child() {
        let document = document("local value = alpha.beta + 1\n", LuaTarget::Lua54);
        let selection = selection_for(&document, Position::new(0, 20));
        let mut current = &selection;
        while let Some(parent) = current.parent.as_deref() {
            assert!(
                parent.range.start <= current.range.start && current.range.end <= parent.range.end,
                "LSP: `parent.range` must contain `this.range`, got child {:?} with parent {:?}",
                current.range,
                parent.range
            );
            current = parent;
        }
    }
}
