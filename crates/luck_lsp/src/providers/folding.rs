//! Folding ranges. Folds function bodies, do-end blocks, control-flow
//! blocks, and table constructors.

use luck_ast::expr::Expression;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;
use luck_token::Span;
use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind};

use crate::backend::DocumentState;

#[must_use]
pub fn folding_ranges(doc: &DocumentState) -> Vec<FoldingRange> {
    let mut collector = Folder {
        doc,
        out: Vec::new(),
    };
    collector.visit_block(&doc.parsed.block);
    collector.out
}

struct Folder<'a> {
    doc: &'a DocumentState,
    out: Vec<FoldingRange>,
}

impl Folder<'_> {
    fn push(&mut self, span: Span, kind: Option<FoldingRangeKind>) {
        let start = self.doc.line_index.position(&self.doc.text, span.start);
        let end = self.doc.line_index.position(&self.doc.text, span.end);
        if end.line <= start.line {
            return;
        }
        self.out.push(FoldingRange {
            start_line: start.line,
            start_character: Some(start.character),
            end_line: end.line,
            end_character: Some(end.character),
            kind,
            collapsed_text: None,
        });
    }
}

impl<'ast> Visitor<'ast> for Folder<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::DoBlock(b) => self.push(b.span, None),
            Statement::WhileLoop(b) => self.push(b.span, None),
            Statement::RepeatLoop(b) => self.push(b.span, None),
            Statement::IfStatement(i) => self.push(i.span, None),
            Statement::NumericFor(b) => self.push(b.span, None),
            Statement::GenericFor(b) => self.push(b.span, None),
            Statement::FunctionDecl(d) => self.push(d.span, None),
            Statement::LocalFunction(l) => self.push(l.span, None),
            _ => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::FunctionDef(def) => self.push(def.span, None),
            Expression::TableConstructor(t) => self.push(t.span, None),
            _ => {}
        }
        self.walk_expression(expr);
    }
}
