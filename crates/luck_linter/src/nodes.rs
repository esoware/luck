//! Flat tables of every `Statement`, `LastStatement`, and `Expression` node,
//! and of every `Block`, each in the default visitor's pre-order.
//!
//! The bus iterates the node table once, and rules that check each block
//! read the block table, instead of each rule re-walking the tree.

use luck_ast::{Block, Expression, LastStatement, Statement, Visitor};

use crate::node_type::{NodeType, NodeTypeSet};
use crate::uninitialized::UninitializedLocals;

/// Borrowed view of one AST node, as dispatched to node rules.
#[derive(Debug, Clone, Copy)]
pub(crate) enum NodeKind<'ast> {
    Statement(&'ast Statement),
    LastStatement(&'ast LastStatement),
    Expression(&'ast Expression),
}

/// One AST node, with its type computed once for the bus's buckets.
#[derive(Debug)]
pub(crate) struct AstNode<'ast> {
    pub kind: NodeKind<'ast>,
    pub node_type: NodeType,
}

/// The flat node and block tables for one chunk.
#[derive(Debug)]
pub(crate) struct Nodes<'ast> {
    table: Vec<AstNode<'ast>>,
    blocks: Vec<&'ast Block>,
    uninitialized_locals: UninitializedLocals,
    present_types: NodeTypeSet,
}

impl<'ast> Nodes<'ast> {
    /// Collects the tables for `block`, in default [`Visitor`] order.
    pub(crate) fn collect(block: &'ast Block) -> Self {
        let mut collector = NodeCollector {
            nodes: Self {
                table: Vec::new(),
                blocks: Vec::new(),
                uninitialized_locals: UninitializedLocals::default(),
                present_types: NodeTypeSet::new(),
            },
        };
        collector.visit_block(block);
        collector.nodes.uninitialized_locals.sort();
        collector.nodes
    }

    pub(crate) fn as_slice(&self) -> &[AstNode<'ast>] {
        &self.table
    }

    /// Every block, the chunk's own first.
    pub(crate) fn blocks(&self) -> &[&'ast Block] {
        &self.blocks
    }

    pub(crate) const fn uninitialized_locals(&self) -> &UninitializedLocals {
        &self.uninitialized_locals
    }

    pub(crate) const fn contains_any(&self, node_types: NodeTypeSet) -> bool {
        self.present_types.intersects(node_types)
    }
}

struct NodeCollector<'ast> {
    nodes: Nodes<'ast>,
}

impl<'ast> NodeCollector<'ast> {
    fn push_node(&mut self, kind: NodeKind<'ast>, node_type: NodeType) {
        self.nodes.present_types.insert(node_type);
        self.nodes.table.push(AstNode { kind, node_type });
    }
}

impl<'ast> Visitor<'ast> for NodeCollector<'ast> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.nodes.blocks.push(block);
        self.walk_block(block);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        self.push_node(NodeKind::Statement(stmt), NodeType::of_stmt(stmt));
        self.nodes.uninitialized_locals.record(stmt);
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.push_node(NodeKind::Expression(expr), NodeType::of_expr(expr));
        self.walk_expr(expr);
    }

    fn visit_last_stmt(&mut self, last: &'ast LastStatement) {
        self.push_node(NodeKind::LastStatement(last), NodeType::of_last_stmt(last));
        self.walk_last_stmt(last);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;

    #[test]
    fn table_matches_visitor_preorder() {
        let parse = luck_parser::parse("local a = 1 + 2\nprint(a)", LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let nodes = Nodes::collect(&parse.block);
        let node_types: Vec<NodeType> =
            nodes.as_slice().iter().map(|node| node.node_type).collect();
        assert_eq!(
            node_types,
            vec![
                NodeType::LocalAssignment,
                NodeType::BinaryOp,
                NodeType::Number,
                NodeType::Number,
                NodeType::FunctionCallStmt,
                NodeType::Var,
                NodeType::Var,
            ]
        );
        assert!(nodes.contains_any(NodeTypeSet::from_types(&[NodeType::BinaryOp])));
        assert!(!nodes.contains_any(NodeTypeSet::from_types(&[NodeType::WhileLoop])));
    }

    #[test]
    fn blocks_match_visitor_preorder() {
        let source = "do local a = 1 end\nlocal function f()\n  if a then print(a) else end\nend";
        let parse = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let nodes = Nodes::collect(&parse.block);
        assert_eq!(nodes.blocks().first(), Some(&&parse.block));
        // The chunk, the `do` body, the function body, then the `if` and
        // `else` bodies.
        let stmt_counts: Vec<usize> =
            nodes.blocks().iter().map(|block| block.stmts.len()).collect();
        assert_eq!(stmt_counts, [2, 1, 1, 1, 0]);
    }
}
