//! Folds function bodies, do-end blocks, control-flow blocks, and table
//! constructors.

use luck_ast::{Expression, Statement, Visitor};
use luck_token::Span;
use tower_lsp::lsp_types::FoldingRange;

use crate::document::DocumentState;

pub(crate) fn ranges(document: &DocumentState) -> Vec<FoldingRange> {
    let mut folder = Folder { document, ranges: Vec::new() };
    folder.visit_block(&document.parsed.block);
    folder.ranges
}

struct Folder<'document> {
    document: &'document DocumentState,
    ranges: Vec<FoldingRange>,
}

impl Folder<'_> {
    fn push(&mut self, span: Span) {
        let start = self.document.position(span.start);
        let end = self.document.position(span.end);
        if end.line <= start.line {
            return;
        }
        self.ranges.push(FoldingRange {
            start_line: start.line,
            start_character: Some(start.character),
            end_line: end.line,
            end_character: Some(end.character),
            kind: None,
            collapsed_text: None,
        });
    }
}

impl<'ast> Visitor<'ast> for Folder<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_) => self.push(stmt.span()),
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::FunctionDef(_) | Expression::TableConstructor(_) => self.push(expr.span()),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => {}
        }
        self.walk_expression(expr);
    }
}
