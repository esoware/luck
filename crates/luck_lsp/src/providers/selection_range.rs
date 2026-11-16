use luck_ast::{Expression, LastStatement, Statement, SuffixedExpression, Visitor};
use luck_token::{Associativity, Span};
use tower_lsp_server::ls_types::{Position, SelectionRange};

use crate::document::DocumentState;
use crate::providers::cursor::touches;

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

    /// A run steps out through each pair it groups into, as if it nested.
    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.record(expr.span());
        if let Expression::BinaryOp(run) = expr {
            let operands = || run.links.iter().map(|link| &link.operand);
            let inner_count = run.links.len().saturating_sub(1);
            if run.precedence().1 == Associativity::Left {
                let start = run.first.span().start;
                for operand in operands().take(inner_count).rev() {
                    self.record(Span::new(start, operand.span().end));
                }
            } else {
                let end = run.links.last().operand.span().end;
                for operand in operands().take(inner_count) {
                    self.record(Span::new(operand.span().start, end));
                }
            }
        }
        self.walk_expr(expr);
    }

    fn visit_suffixed(&mut self, chain: &'ast SuffixedExpression) {
        for count in (1..chain.suffixes.len()).rev() {
            self.record(chain.head(count).span());
        }
        self.walk_suffixed(chain);
    }
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;
    use tower_lsp_server::ls_types::Range;

    use super::*;

    /// The selection at `offset` on the first line, innermost first.
    fn ranges_at(source: &str, offset: u32) -> Vec<Range> {
        let document =
            DocumentState::new(source.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        assert!(document.parsed.errors.is_empty(), "{:?}", document.parsed.errors);
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
