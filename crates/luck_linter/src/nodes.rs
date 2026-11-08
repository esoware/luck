//! Flat table of every `Statement`, `LastStatement`, and `Expression` node,
//! in the default visitor's pre-order.
//!
//! The bus iterates this once instead of each rule re-walking the tree.

use luck_ast::{
    Block, Expression, LastStatement, NodeKind, NodeType, NodeTypeSet, Statement, Visitor,
};

/// One AST node, with its type computed once for the bus's buckets.
#[derive(Debug)]
pub(crate) struct AstNode<'ast> {
    pub kind: NodeKind<'ast>,
    pub node_type: NodeType,
}

/// The flat node table for one chunk.
#[derive(Debug)]
pub(crate) struct Nodes<'ast> {
    nodes: Vec<AstNode<'ast>>,
    present_types: NodeTypeSet,
}

impl<'ast> Nodes<'ast> {
    /// Collects the node table for `block`, in default [`Visitor`] order.
    pub(crate) fn collect(block: &'ast Block) -> Self {
        let mut collector = NodeCollector { nodes: Vec::new(), present_types: NodeTypeSet::new() };
        collector.visit_block(block);
        Self { nodes: collector.nodes, present_types: collector.present_types }
    }

    pub(crate) fn as_slice(&self) -> &[AstNode<'ast>] {
        &self.nodes
    }

    pub(crate) const fn contains_any(&self, node_types: NodeTypeSet) -> bool {
        self.present_types.intersects(node_types)
    }
}

struct NodeCollector<'ast> {
    nodes: Vec<AstNode<'ast>>,
    present_types: NodeTypeSet,
}

impl<'ast> NodeCollector<'ast> {
    fn push_node(&mut self, kind: NodeKind<'ast>) {
        let node_type = kind.node_type();
        self.present_types.insert(node_type);
        self.nodes.push(AstNode { kind, node_type });
    }
}

impl<'ast> Visitor<'ast> for NodeCollector<'ast> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        self.push_node(NodeKind::Statement(stmt));
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        self.push_node(NodeKind::Expression(expr));
        self.walk_expression(expr);
    }

    fn visit_last_statement(&mut self, last: &'ast LastStatement) {
        self.push_node(NodeKind::LastStatement(last));
        self.walk_last_statement(last);
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
}
