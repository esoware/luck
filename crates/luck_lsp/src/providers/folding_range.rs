use ls_types::FoldingRange;
use luck_ast::{
    DeclaredExternType, FunctionBody, GetSpan as _, Statement, TableConstructor, Visit,
};
use luck_token::Span;

use crate::document::DocumentState;
use crate::line_index::Positions;

pub(crate) fn folding_ranges(document: &DocumentState) -> Vec<FoldingRange> {
    let mut folder = Folder { document, starts: document.positions(), ranges: Vec::new() };
    folder.visit_block(&document.parse().chunk);
    folder.ranges
}

struct Folder<'document> {
    document: &'document DocumentState,
    /// The walk meets each fold before those inside it, so their starts come
    /// in order.
    starts: Positions<'document>,
    ranges: Vec<FoldingRange>,
}

impl Folder<'_> {
    fn push(&mut self, span: Span) {
        let start = self.starts.position(span.start);
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

impl<'ast> Visit<'ast> for Folder<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
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
            | Statement::Return(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Declaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    fn visit_declared_extern_type(&mut self, extern_type: &'ast DeclaredExternType<'ast>) {
        self.push(extern_type.span);
        self.walk_declared_extern_type(extern_type);
    }

    // Every function form reaches this: declarations, `local` functions, type
    // functions, and anonymous functions.
    fn visit_function_body(&mut self, function_body: &'ast FunctionBody<'ast>) {
        self.push(function_body.span);
        self.walk_function_body(function_body);
    }

    fn visit_table_constructor(&mut self, table_constructor: &'ast TableConstructor<'ast>) {
        self.push(table_constructor.span);
        self.walk_table_constructor(table_constructor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::document;

    fn folded_lines(text: &str) -> Vec<(u32, u32)> {
        let document = document(text);
        folding_ranges(&document).iter().map(|fold| (fold.start_line, fold.end_line)).collect()
    }

    #[test]
    fn a_table_call_argument_folds() {
        let folds = folded_lines(
            "\
setup {
  a = 1,
}
",
        );
        assert_eq!(folds, vec![(0, 2)], "`f {{...}}` is a table constructor like any other");
    }

    #[test]
    fn a_type_function_folds() {
        let folds = folded_lines(
            "\
type function f(t)
  return t
end
",
        );
        assert_eq!(folds, vec![(0, 2)], "a `type function` body folds like any other");
    }
}
