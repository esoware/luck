use luck_ast::{Expression, LastStatement, Statement, Visitor};
use luck_token::Span;
use tower_lsp_server::ls_types::{Position, SelectionRange};

use crate::document::DocumentState;
use crate::providers::cursor::touches;

/// How many of the innermost enclosing spans reach the editor. The `parent`
/// chain's drop and serialization recurse, so a long operator chain must not
/// become an equally long list.
const MAX_SELECTION_DEPTH: usize = 256;

pub(crate) fn selection_ranges(
    document: &DocumentState,
    positions: &[Position],
) -> Vec<SelectionRange> {
    positions.iter().map(|&position| selection_for(document, position)).collect()
}

fn selection_for(document: &DocumentState, position: Position) -> SelectionRange {
    let offset = document.offset(position);
    let mut collector = SpanStack { offset, stack: Vec::new() };
    collector.visit_block(&document.parsed.block);
    let first_kept = collector.stack.len().saturating_sub(MAX_SELECTION_DEPTH);
    collector.stack.drain(..first_kept);

    // The returned link is the narrowest span and each `parent` the next one
    // out, so the chain folds from the outside in.
    let mut chain: Option<SelectionRange> = None;
    for &span in &collector.stack {
        chain = Some(SelectionRange { range: document.range(span), parent: chain.map(Box::new) });
    }
    chain.unwrap_or_else(|| SelectionRange {
        range: document.range(Span::new(offset, offset)),
        parent: None,
    })
}

struct SpanStack {
    offset: u32,
    stack: Vec<Span>,
}

impl SpanStack {
    fn record(&mut self, span: Span) {
        if !touches(span, self.offset) {
            return;
        }
        // At a boundary between siblings the offset touches both, so the
        // later one replaces the earlier one's whole chain.
        while self.stack.last().is_some_and(|top| span.start < top.start || top.end < span.end) {
            self.stack.pop();
        }
        self.stack.push(span);
    }
}

impl<'ast> Visitor<'ast> for SpanStack {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        self.record(stmt.span());
        self.walk_stmt(stmt);
    }

    fn visit_last_stmt(&mut self, last: &'ast LastStatement) {
        self.record(last.span());
        self.walk_last_stmt(last);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.record(expr.span());
        self.walk_expr(expr);
    }
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;
    use tower_lsp_server::ls_types::Range;

    use super::*;

    #[test]
    fn a_selection_chain_at_a_statement_boundary_is_nested() {
        // Offset 3 ends `f()` and starts `g()`. Either direction of nesting
        // passes, so this is independent of the chain's order.
        let document =
            DocumentState::new("f()g()\n".to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        assert!(document.parsed.errors.is_empty(), "{:?}", document.parsed.errors);
        let selection = selection_for(&document, Position::new(0, 3));
        let mut ranges = vec![selection.range];
        let mut current = &selection;
        while let Some(parent) = current.parent.as_deref() {
            ranges.push(parent.range);
            current = parent;
        }
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
    fn a_selection_range_parent_contains_its_child() {
        let document = DocumentState::new(
            "local value = alpha.beta + 1\n".to_owned(),
            1,
            LuaTarget::Lua54,
            FxHashSet::default(),
        );
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
