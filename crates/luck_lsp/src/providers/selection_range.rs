//! Selection ranges. For each requested position, return the chain of
//! enclosing AST node spans so the editor can expand selection in
//! semantic increments.

use luck_ast::expr::Expression;
use luck_ast::shared::Block;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;
use luck_token::Span;
use tower_lsp::lsp_types::{Position, SelectionRange, SelectionRangeParams};

use crate::backend::DocumentState;

#[must_use]
pub fn selection_ranges(doc: &DocumentState, params: &SelectionRangeParams) -> Vec<SelectionRange> {
    params
        .positions
        .iter()
        .map(|pos| selection_for(doc, *pos))
        .collect()
}

fn selection_for(doc: &DocumentState, position: Position) -> SelectionRange {
    let offset = doc.line_index.offset(&doc.text, position);
    let mut collector = SpanStack {
        offset,
        stack: Vec::new(),
    };
    collector.visit_block(&doc.parsed.block);

    let mut chain: Option<SelectionRange> = None;
    for span in collector.stack.iter().rev() {
        let range = doc.line_index.range(&doc.text, span.start, span.end);
        chain = Some(SelectionRange {
            range,
            parent: chain.map(Box::new),
        });
    }
    chain.unwrap_or(SelectionRange {
        range: doc.line_index.range(&doc.text, offset, offset),
        parent: None,
    })
}

struct SpanStack {
    offset: u32,
    stack: Vec<Span>,
}

impl SpanStack {
    fn record(&mut self, span: Span) {
        if self.offset >= span.start && self.offset <= span.end {
            self.stack.push(span);
        }
    }
}

impl Visitor for SpanStack {
    fn visit_statement(&mut self, stmt: &Statement) {
        let span = stmt.span();
        self.record(span);
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &Expression) {
        let span = expr.span();
        self.record(span);
        self.walk_expression(expr);
    }

    fn visit_block(&mut self, block: &Block) {
        self.walk_block(block);
    }
}
