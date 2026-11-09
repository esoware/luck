//! Folds function bodies, do-end blocks, control-flow blocks, and table
//! constructors.

use luck_ast::{FunctionBody, Statement, TableConstructor, Visitor};
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
            | Statement::GenericFor(_) => self.push(stmt.span()),
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
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

    // Every function form reaches this: declarations, `local` and `global`
    // functions, Luau type functions, and anonymous functions.
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.push(body.span);
        self.walk_function_body(body);
    }

    fn visit_table_constructor(&mut self, table: &'ast TableConstructor) {
        self.push(table.span);
        self.walk_table_constructor(table);
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
    fn a_table_call_argument_folds() {
        let folds = folded_lines("setup {\n  a = 1,\n}\n", LuaTarget::Lua54);
        assert_eq!(folds, vec![(0, 2)], "`f {{...}}` is a table constructor like any other");
    }

    #[test]
    fn a_luau_type_function_folds() {
        let folds = folded_lines("type function f(t)\n  return t\nend\n", LuaTarget::Luau);
        assert_eq!(folds, vec![(0, 2)], "a `type function` body folds like any other");
    }

    #[test]
    fn a_lua55_global_function_folds() {
        let folds = folded_lines("global function f()\n  return 1\nend\n", LuaTarget::Lua55);
        assert_eq!(folds, vec![(0, 2)], "a `global function` body folds like any other");
    }
}
