//! For each requested position, return the chain of enclosing AST node spans
//! so the editor can expand selection in semantic increments.

use luck_ast::{Expression, Statement, Visitor};
use luck_token::Span;
use tower_lsp::lsp_types::{Position, SelectionRange};

use crate::document::DocumentState;
use crate::providers::cursor::touches;

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

    let mut chain: Option<SelectionRange> = None;
    for &span in collector.stack.iter().rev() {
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
        if touches(span, self.offset) {
            self.stack.push(span);
        }
    }
}

impl<'ast> Visitor<'ast> for SpanStack {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        self.record(stmt.span());
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.record(expr.span());
        self.walk_expr(expr);
    }
}
