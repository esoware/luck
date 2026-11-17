use luck_ast::{
    Block, Expression, FunctionBody, LastStatement, Statement, SuffixedExpression, TableConstructor,
};

/// Sees each node as [`analyze_with`](crate::analyze_with) reaches it, so a
/// consumer that needs its own view of every node, such as the linter's node
/// table, builds it in the same walk instead of walking the tree again.
///
/// The walk is in evaluation order, which is source order except that an
/// assignment's values come before its targets, and a function's signature
/// annotations before its body. Each node comes before the nodes inside it.
pub trait NodeSink<'ast> {
    fn block(&mut self, _block: &'ast Block<'ast>) {}
    /// Before the body's block.
    fn function_body(&mut self, _body: &'ast FunctionBody<'ast>) {}
    fn stmt(&mut self, _stmt: &'ast Statement<'ast>) {}
    fn last_stmt(&mut self, _last: &'ast LastStatement<'ast>) {}
    /// Every expression, a chain or a table included, before
    /// [`Self::chain`] or [`Self::table`] gets it.
    fn expr(&mut self, _expr: &'ast Expression<'ast>) {}
    /// A chain in an expression, a call statement, or, when `is_target`, an
    /// assignment target.
    fn chain(&mut self, _chain: &'ast SuffixedExpression<'ast>, _is_target: bool) {}
    /// A table in an expression or as a `f{...}` argument.
    fn table(&mut self, _table: &'ast TableConstructor<'ast>) {}
}

impl NodeSink<'_> for () {}
