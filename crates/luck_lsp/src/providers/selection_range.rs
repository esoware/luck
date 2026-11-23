use ls_types::{Position, SelectionRange};
use luck_ast::{Block, Expression, GetSpan as _, Statement, SuffixedExpression, Visit};
use luck_parser::Groupings;
use luck_token::{Associativity, Span};

use crate::document::DocumentState;
use crate::providers::cursor::{statements_touching, touches};

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
    let parse = document.parse();
    let mut collector = SpanStack { offset, groupings: &parse.groupings, stack: Vec::new() };
    collector.visit_block(&parse.chunk.block);
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

struct SpanStack<'ast> {
    offset: u32,
    groupings: &'ast Groupings,
    stack: Vec<Span>,
}

impl SpanStack<'_> {
    /// `span` with the parentheses the expression it's the span of is in.
    fn extent(&self, span: Span) -> Span {
        self.groupings.around(span).unwrap_or(span)
    }

    /// Records an expression's span, inside the parentheses it's in, if any.
    fn record_grouped(&mut self, span: Span) {
        if let Some(parens) = self.groupings.around(span) {
            self.record(parens);
        }
        self.record(span);
    }

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

impl<'ast> Visit<'ast> for SpanStack<'_> {
    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        for stmt in statements_touching(block, self.offset) {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        self.record(stmt.span());
        self.walk_stmt(stmt);
    }

    /// A run steps out through each pair it groups into, as if it nested.
    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if !touches(self.extent(expr.span()), self.offset) {
            return;
        }
        self.record_grouped(expr.span());
        if let Expression::BinaryOp(run) = expr {
            let operands = || run.links.iter().map(|link| &link.operand);
            let inner_count = run.links.len().saturating_sub(1);
            if run.precedence().1 == Associativity::Left {
                let start = self.extent(run.first.span()).start;
                for operand in operands().take(inner_count).rev() {
                    self.record(Span::new(start, self.extent(operand.span()).end));
                }
            } else {
                let end = self.extent(run.links.last().operand.span()).end;
                for operand in operands().take(inner_count) {
                    self.record(Span::new(self.extent(operand.span()).start, end));
                }
            }
        }
        self.walk_expr(expr);
    }

    /// Each head is the chain it was before the next suffix joined it, in the
    /// parentheses it was in.
    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        let mut heads = Vec::new();
        let mut start = self.extent(suffixed_expr.prefix.span()).start;
        let inner_count = suffixed_expr.suffixes.len().saturating_sub(1);
        for suffix in suffixed_expr.suffixes.iter().take(inner_count) {
            let head = Span::new(start, suffix.span().end);
            heads.push(head);
            if let Some(parens) = self.groupings.around(head) {
                heads.push(parens);
                start = parens.start;
            }
        }
        for &head in heads.iter().rev() {
            self.record(head);
        }
        self.walk_suffixed_expr(suffixed_expr);
    }
}

#[cfg(test)]
mod tests {
    use ls_types::Range;
    use luck_core::LuaTarget;

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
