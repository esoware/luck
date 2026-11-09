//! Folds function bodies, do-end blocks, control-flow blocks, and table
//! constructors.

use luck_ast::{Expression, Statement, Visitor};
use luck_token::Span;
use tower_lsp::lsp_types::FoldingRange;

use crate::document::DocumentState;

pub(crate) fn folding_ranges(document: &DocumentState) -> Vec<FoldingRange> {
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
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
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
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
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
        self.walk_expr(expr);
    }
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;

    use super::*;

    fn folded_lines(text: &str, target: LuaTarget) -> Vec<(u32, u32)> {
        let document = DocumentState::new(text.to_owned(), 1, target, FxHashSet::default());
        folding_ranges(&document).iter().map(|fold| (fold.start_line, fold.end_line)).collect()
    }

    #[test]
    fn bug_a_table_call_argument_folds() {
        let folds = folded_lines("setup {\n  a = 1,\n}\n", LuaTarget::Lua54);
        assert_eq!(folds, vec![(0, 2)], "`f {{...}}` is a table constructor like any other");
    }

    #[test]
    fn bug_a_luau_type_function_folds() {
        let folds = folded_lines("type function f(t)\n  return t\nend\n", LuaTarget::Luau);
        assert_eq!(folds, vec![(0, 2)], "a `type function` body folds like any other");
    }

    #[test]
    fn bug_a_lua55_global_function_folds() {
        let folds = folded_lines("global function f()\n  return 1\nend\n", LuaTarget::Lua55);
        assert_eq!(folds, vec![(0, 2)], "a `global function` body folds like any other");
    }
}
