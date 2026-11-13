//! The bus iterates the node table once, and rules that check each block
//! read the block table, instead of each rule re-walking the tree.

use core::ptr;

use luck_ast::{
    Block, Expression, FunctionBody, LastStatement, Statement, Suffix, SuffixedExpression,
    TableConstructor, Var, Visitor,
};

use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::ChainPosition;
use crate::uninitialized::UninitializedLocals;

#[derive(Debug, Clone, Copy)]
pub(crate) enum NodeKind<'ast> {
    Statement(&'ast Statement),
    LastStatement(&'ast LastStatement),
    /// Never an `Expression::TableConstructor`, which is a [`Self::Table`], or
    /// an `Expression::Suffixed`, which is a [`Self::Chain`].
    Expression(&'ast Expression),
    /// A table in an expression or as a `f{...}` argument.
    Table(&'ast TableConstructor),
    /// A chain in an expression, a call statement, or an assignment target.
    Chain(&'ast SuffixedExpression, ChainPosition),
    /// The call at `index` in a chain, other than the one a call statement
    /// makes. A `u32` keeps the node at three words, and no source has more
    /// suffixes than bytes.
    Call {
        chain: &'ast SuffixedExpression,
        index: u32,
    },
}

#[derive(Debug)]
pub(crate) struct AstNode<'ast> {
    pub kind: NodeKind<'ast>,
    pub node_type: NodeType,
}

#[derive(Debug)]
pub(crate) struct Nodes<'ast> {
    table: Vec<AstNode<'ast>>,
    blocks: Vec<&'ast Block>,
    function_blocks: Vec<&'ast Block>,
    uninitialized_locals: UninitializedLocals,
    present_types: NodeTypeSet,
}

impl<'ast> Nodes<'ast> {
    pub(crate) fn collect(block: &'ast Block) -> Self {
        let mut collector = NodeCollector {
            nodes: Self {
                table: Vec::new(),
                blocks: Vec::new(),
                function_blocks: vec![block],
                uninitialized_locals: UninitializedLocals::default(),
                present_types: NodeTypeSet::new(),
            },
            statement_chain: None,
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

    /// The chunk's block, then every function's, nested ones included.
    pub(crate) fn function_blocks(&self) -> &[&'ast Block] {
        &self.function_blocks
    }

    pub(crate) const fn uninitialized_locals(&self) -> &UninitializedLocals {
        &self.uninitialized_locals
    }

    pub(crate) const fn contains_any(&self, node_types: NodeTypeSet) -> bool {
        self.present_types.intersects(node_types)
    }
}

/// Each statement of `block` with the one after it, ignoring `;`.
pub(crate) fn adjacent_stmts(block: &Block) -> impl Iterator<Item = (&Statement, &Statement)> {
    let stmts = block.stmts.iter().filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_)));
    stmts.clone().zip(stmts.skip(1))
}

struct NodeCollector<'ast> {
    nodes: Nodes<'ast>,
    /// The chain of the call statement being walked, whose last call is that
    /// statement's node.
    statement_chain: Option<&'ast SuffixedExpression>,
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
        if let Statement::FunctionCall(chain) = stmt {
            self.statement_chain = Some(chain);
        }
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if !matches!(expr, Expression::TableConstructor(_) | Expression::Suffixed(_)) {
            self.push_node(NodeKind::Expression(expr), NodeType::of_expr(expr));
        }
        self.walk_expr(expr);
    }

    fn visit_suffixed(&mut self, chain: &'ast SuffixedExpression) {
        self.push_node(NodeKind::Chain(chain, ChainPosition::Value), NodeType::Suffixed);
        let is_statement = self.statement_chain.is_some_and(|statement| ptr::eq(statement, chain));
        let expression_suffixes = if is_statement {
            chain.suffixes.len().saturating_sub(1)
        } else {
            chain.suffixes.len()
        };
        for (index, suffix) in (0u32..).zip(chain.suffixes.iter().take(expression_suffixes)) {
            if matches!(suffix, Suffix::Call(_)) {
                self.push_node(NodeKind::Call { chain, index }, NodeType::FunctionCallExpr);
            }
        }
        self.walk_suffixed(chain);
    }

    fn visit_assignment_target(&mut self, target: &'ast Var) {
        if let Var::Suffixed(chain) = target {
            let kind = NodeKind::Chain(chain, ChainPosition::AssignmentTarget);
            self.push_node(kind, NodeType::Suffixed);
            self.walk_suffixed(chain);
        }
    }

    fn visit_table_constructor(&mut self, table: &'ast TableConstructor) {
        self.push_node(NodeKind::Table(table), NodeType::TableConstructor);
        self.walk_table_constructor(table);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.nodes.function_blocks.push(&body.block);
        self.walk_function_body(body);
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
                NodeType::Suffixed,
                NodeType::Name,
                NodeType::Name,
            ]
        );
        assert!(nodes.contains_any(NodeTypeSet::from_types(&[NodeType::BinaryOp])));
        assert!(!nodes.contains_any(NodeTypeSet::from_types(&[NodeType::WhileLoop])));
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn node_is_three_words() {
        assert_eq!(size_of::<AstNode<'_>>(), 24);
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
