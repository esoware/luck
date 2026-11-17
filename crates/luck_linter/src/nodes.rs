//! The semantic walk builds the node table, which the bus iterates once and
//! whole-document rules read with the block tables, so no rule walks the
//! tree again.

use core::ptr;

use luck_ast::{
    Block, Call, Expression, FunctionBody, LastStatement, Statement, Suffix, SuffixedExpression,
    TableConstructor,
};
use luck_semantic::NodeSink;
use luck_token::offset_to_index;

use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, ChainPosition};
use crate::uninitialized::UninitializedLocals;

#[derive(Debug, Clone, Copy)]
pub(crate) enum NodeKind<'ast> {
    Statement(&'ast Statement<'ast>),
    LastStatement(&'ast LastStatement<'ast>),
    /// Never an `Expression::TableConstructor`, which is a [`Self::Table`], or
    /// an `Expression::Suffixed`, which is a [`Self::Chain`].
    Expression(&'ast Expression<'ast>),
    /// A table in an expression or as a `f{...}` argument.
    Table(&'ast TableConstructor<'ast>),
    /// A chain in an expression, a call statement, or an assignment target.
    Chain(&'ast SuffixedExpression<'ast>, ChainPosition),
    /// The call at `index` in a chain, other than the one a call statement
    /// makes. A `u32` keeps the node at three words, and no source has more
    /// suffixes than bytes.
    Call {
        chain: &'ast SuffixedExpression<'ast>,
        index: u32,
    },
}

#[derive(Debug)]
pub(crate) struct AstNode<'ast> {
    pub kind: NodeKind<'ast>,
    pub node_type: NodeType,
}

impl<'ast> AstNode<'ast> {
    /// For a call node, the chain the call is in, the index of its suffix
    /// there, and where the call stands.
    pub(crate) fn chain_call(
        &self,
    ) -> Option<(&'ast SuffixedExpression<'ast>, usize, CallPosition)> {
        match self.kind {
            NodeKind::Statement(Statement::FunctionCall(chain)) => {
                Some((chain, chain.suffixes.len().saturating_sub(1), CallPosition::Statement))
            }
            NodeKind::Call { chain, index } => {
                Some((chain, usize::try_from(index).ok()?, CallPosition::Expression))
            }
            NodeKind::Statement(_)
            | NodeKind::LastStatement(_)
            | NodeKind::Expression(_)
            | NodeKind::Table(_)
            | NodeKind::Chain(..) => None,
        }
    }

    /// The call a call node makes.
    pub(crate) fn call(&self) -> Option<Call<'ast>> {
        let (chain, index, _) = self.chain_call()?;
        Call::new(chain.head(index), chain.suffixes.get(index)?)
    }
}

#[derive(Debug)]
pub(crate) struct Nodes<'ast> {
    table: Vec<AstNode<'ast>>,
    blocks: Vec<&'ast Block<'ast>>,
    function_blocks: Vec<&'ast Block<'ast>>,
    uninitialized_locals: UninitializedLocals,
    present_types: NodeTypeSet,
}

impl<'ast> Nodes<'ast> {
    pub(crate) fn as_slice(&self) -> &[AstNode<'ast>] {
        &self.table
    }

    /// Every block, the chunk's own first.
    pub(crate) fn blocks(&self) -> &[&'ast Block<'ast>] {
        &self.blocks
    }

    /// The chunk's block, then every function's, nested ones included.
    pub(crate) fn function_blocks(&self) -> &[&'ast Block<'ast>] {
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
pub(crate) fn adjacent_stmts<'ast>(
    block: &'ast Block<'ast>,
) -> impl Iterator<Item = (&'ast Statement<'ast>, &'ast Statement<'ast>)> {
    let stmts = block.stmts.iter().filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_)));
    stmts.clone().zip(stmts.skip(1))
}

/// Builds a document's [`Nodes`] as the semantic walk hands it each node.
pub(crate) struct NodeCollector<'ast> {
    nodes: Nodes<'ast>,
    /// The call statement being walked, and its chain. Its node is the
    /// chain's last call, and follows the chain's other calls, so the bus
    /// resolves them front to back.
    call_statement: Option<(&'ast Statement<'ast>, &'ast SuffixedExpression<'ast>)>,
}

impl<'ast> NodeCollector<'ast> {
    pub(crate) fn new(block: &'ast Block<'ast>) -> Self {
        Self {
            nodes: Nodes {
                // Code takes about a node per 16 bytes.
                table: Vec::with_capacity(offset_to_index(block.span.len() >> 4)),
                blocks: Vec::new(),
                function_blocks: vec![block],
                uninitialized_locals: UninitializedLocals::default(),
                present_types: NodeTypeSet::new(),
            },
            call_statement: None,
        }
    }

    pub(crate) fn finish(mut self) -> Nodes<'ast> {
        self.nodes.uninitialized_locals.sort();
        self.nodes
    }

    fn push_node(&mut self, kind: NodeKind<'ast>, node_type: NodeType) {
        self.nodes.present_types.insert(node_type);
        self.nodes.table.push(AstNode { kind, node_type });
    }
}

impl<'ast> NodeSink<'ast> for NodeCollector<'ast> {
    fn block(&mut self, block: &'ast Block<'ast>) {
        self.nodes.blocks.push(block);
    }

    fn function_body(&mut self, body: &'ast FunctionBody<'ast>) {
        self.nodes.function_blocks.push(&body.block);
    }

    fn stmt(&mut self, stmt: &'ast Statement<'ast>) {
        if let Statement::FunctionCall(chain) = stmt {
            self.call_statement = Some((stmt, chain));
        } else {
            self.push_node(NodeKind::Statement(stmt), NodeType::of_stmt(stmt));
        }
        self.nodes.uninitialized_locals.record(stmt);
    }

    fn last_stmt(&mut self, last: &'ast LastStatement<'ast>) {
        self.push_node(NodeKind::LastStatement(last), NodeType::of_last_stmt(last));
    }

    fn expr(&mut self, expr: &'ast Expression<'ast>) {
        if !matches!(expr, Expression::TableConstructor(_) | Expression::Suffixed(_)) {
            self.push_node(NodeKind::Expression(expr), NodeType::of_expr(expr));
        }
    }

    fn chain(&mut self, chain: &'ast SuffixedExpression<'ast>, is_target: bool) {
        if is_target {
            let kind = NodeKind::Chain(chain, ChainPosition::AssignmentTarget);
            self.push_node(kind, NodeType::Suffixed);
            return;
        }
        self.push_node(NodeKind::Chain(chain, ChainPosition::Value), NodeType::Suffixed);
        let statement = self
            .call_statement
            .filter(|&(_, statement_chain)| ptr::eq(statement_chain, chain))
            .map(|(stmt, _)| stmt);
        let expression_suffixes = if statement.is_some() {
            chain.suffixes.len().saturating_sub(1)
        } else {
            chain.suffixes.len()
        };
        for (index, suffix) in (0u32..).zip(chain.suffixes.iter().take(expression_suffixes)) {
            if matches!(suffix, Suffix::Call(_) | Suffix::MethodCall(_)) {
                self.push_node(NodeKind::Call { chain, index }, NodeType::FunctionCallExpr);
            }
        }
        if let Some(stmt) = statement {
            self.push_node(NodeKind::Statement(stmt), NodeType::FunctionCallStmt);
        }
    }

    fn table(&mut self, table: &'ast TableConstructor<'ast>) {
        self.push_node(NodeKind::Table(table), NodeType::TableConstructor);
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_token::{LuaVersion, StdlibEnvironment};

    use super::*;

    fn collect<'ast>(block: &'ast Block<'ast>) -> Nodes<'ast> {
        let mut collector = NodeCollector::new(block);
        let _semantic = luck_semantic::analyze_with(
            block,
            LuaVersion::Lua54,
            StdlibEnvironment::Standalone,
            &mut collector,
        );
        collector.finish()
    }

    #[test]
    fn table_is_in_evaluation_order() {
        let allocator = Allocator::new();
        let parse = luck_parser::parse(
            &allocator,
            "\
local a = 1 + 2
print(a)
t.x = a",
            LuaVersion::Lua54,
        );
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let nodes = collect(&parse.block);
        let node_types: Vec<NodeType> =
            nodes.as_slice().iter().map(|node| node.node_type).collect();
        assert_eq!(
            node_types,
            vec![
                NodeType::LocalAssignment,
                NodeType::BinaryOp,
                NodeType::Number,
                NodeType::Number,
                NodeType::Suffixed,
                NodeType::FunctionCallStmt,
                NodeType::Name,
                NodeType::Name,
                NodeType::Assignment,
                NodeType::Name,
                NodeType::Suffixed,
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
    fn blocks_are_in_preorder() {
        let source = "\
do local a = 1 end
local function f()
  if a then print(a) else end
end";
        let allocator = Allocator::new();
        let parse = luck_parser::parse(&allocator, source, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let nodes = collect(&parse.block);
        assert_eq!(nodes.blocks().first().map(|first| first.span), Some(parse.block.span));
        // The chunk, the `do` body, the function body, then the `if` and
        // `else` bodies.
        let stmt_counts: Vec<usize> =
            nodes.blocks().iter().map(|block| block.stmts.len()).collect();
        assert_eq!(stmt_counts, [2, 1, 1, 1, 0]);
        let function_stmt_counts: Vec<usize> =
            nodes.function_blocks().iter().map(|block| block.stmts.len()).collect();
        assert_eq!(function_stmt_counts, [2, 1]);
    }
}
