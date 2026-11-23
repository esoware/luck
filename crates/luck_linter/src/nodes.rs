//! One walk builds the node table, which the bus iterates once and
//! whole-document rules read with the block tables, so no rule walks the
//! tree again.

use core::ptr;

use luck_ast::{
    Assignment, Block, Call, Chunk, CompoundAssignment, Expression, FunctionBody, FunctionDecl,
    GenericFor, GlobalDeclaration, GlobalFunction, GlobalStar, LocalAssignment, LocalFunction,
    NumericFor, Statement, Suffix, SuffixedExpression, TableConstructor, Var, Visit,
};
use luck_token::offset_to_index;

use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, ChainPosition};
use crate::uninitialized::UninitializedLocals;

#[derive(Debug, Clone, Copy)]
pub(crate) enum NodeKind<'ast> {
    Statement(&'ast Statement<'ast>),
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

#[derive(Debug, Default)]
pub(crate) struct Nodes<'ast> {
    table: Vec<AstNode<'ast>>,
    blocks: Vec<&'ast Block<'ast>>,
    function_blocks: Vec<&'ast Block<'ast>>,
    uninitialized_locals: UninitializedLocals,
    present_types: NodeTypeSet,
}

impl<'ast> Nodes<'ast> {
    /// The table of `chunk`'s nodes, in evaluation order, which is source
    /// order except that an assignment's values come before its targets, and
    /// a function's signature annotations before its body. Each node comes
    /// before the nodes inside it.
    pub(crate) fn collect(chunk: &'ast Chunk<'ast>) -> Self {
        let mut collector = NodeCollector::new(chunk);
        collector.visit_block(&chunk.block);
        collector.finish()
    }

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

/// Each statement of `block` with the one after it.
pub(crate) fn adjacent_stmts<'ast>(
    block: &'ast Block<'ast>,
) -> impl Iterator<Item = (&'ast Statement<'ast>, &'ast Statement<'ast>)> {
    block.stmts.iter().zip(block.stmts.iter().skip(1))
}

/// Builds a document's [`Nodes`] in one walk.
struct NodeCollector<'ast> {
    nodes: Nodes<'ast>,
    /// The call statement being walked, and its chain. Its node is the
    /// chain's last call, and follows the chain's other calls, so the bus
    /// resolves them front to back.
    call_statement: Option<(&'ast Statement<'ast>, &'ast SuffixedExpression<'ast>)>,
}

impl<'ast> NodeCollector<'ast> {
    fn new(chunk: &'ast Chunk<'ast>) -> Self {
        let block = &chunk.block;
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

    fn finish(mut self) -> Nodes<'ast> {
        self.nodes.uninitialized_locals.sort();
        self.nodes
    }

    fn push_node(&mut self, kind: NodeKind<'ast>, node_type: NodeType) {
        self.nodes.present_types.insert(node_type);
        self.nodes.table.push(AstNode { kind, node_type });
    }

    /// The annotations of a function's signature, which come before its
    /// body.
    fn visit_signature_annotations(&mut self, body: &'ast FunctionBody<'ast>) {
        for param in &body.params {
            if let Some(annotation) = &param.type_annotation {
                self.visit_type(annotation);
            }
        }
        if let Some(annotation) =
            body.vararg.as_ref().and_then(|vararg| vararg.type_annotation.as_ref())
        {
            self.visit_type(annotation);
        }
        if let Some(annotation) = &body.return_type {
            self.visit_type(annotation);
        }
    }

    /// An indexed or field target only reads its prefix and key.
    fn visit_target(&mut self, var: &'ast Var<'ast>) {
        if let Var::Suffixed(chain) = var {
            self.chain(chain, true);
            self.walk_suffixed_expr(chain);
        }
    }

    fn record_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        if let Statement::FunctionCall(chain) = stmt {
            self.call_statement = Some((stmt, chain));
        } else {
            self.push_node(NodeKind::Statement(stmt), NodeType::of_stmt(stmt));
        }
        self.nodes.uninitialized_locals.record(stmt);
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
}

impl<'ast> Visit<'ast> for NodeCollector<'ast> {
    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        self.nodes.blocks.push(block);
        self.walk_block(block);
    }

    fn visit_function_body(&mut self, function_body: &'ast FunctionBody<'ast>) {
        self.visit_signature_annotations(function_body);
        self.nodes.function_blocks.push(&function_body.block);
        self.visit_block(&function_body.block);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        self.record_stmt(stmt);
        self.walk_stmt(stmt);
    }

    fn visit_assignment(&mut self, assignment: &'ast Assignment<'ast>) {
        for expr in &assignment.values {
            self.visit_expr(expr);
        }
        for var in &assignment.targets {
            self.visit_target(var);
        }
    }

    // Luau
    fn visit_compound_assignment(&mut self, compound_assignment: &'ast CompoundAssignment<'ast>) {
        self.visit_expr(&compound_assignment.expr);
        self.visit_target(&compound_assignment.var);
    }

    fn visit_local_assignment(&mut self, local_assignment: &'ast LocalAssignment<'ast>) {
        for attributed in &local_assignment.names {
            if let Some(annotation) = &attributed.type_annotation {
                self.visit_type(annotation);
            }
        }
        for expr in &local_assignment.exprs {
            self.visit_expr(expr);
        }
    }

    fn visit_local_function(&mut self, local_function: &'ast LocalFunction<'ast>) {
        for attribute in &local_function.attributes {
            self.visit_function_attribute(attribute);
        }
        self.visit_function_body(&local_function.body);
    }

    fn visit_function_decl(&mut self, function_decl: &'ast FunctionDecl<'ast>) {
        for attribute in &function_decl.attributes {
            self.visit_function_attribute(attribute);
        }
        self.visit_function_body(&function_decl.body);
    }

    fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor<'ast>) {
        if let Some(annotation) = &numeric_for.type_annotation {
            self.visit_type(annotation);
        }
        self.visit_expr(&numeric_for.start);
        self.visit_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.visit_expr(step);
        }
        self.visit_block(&numeric_for.block);
    }

    fn visit_generic_for(&mut self, generic_for: &'ast GenericFor<'ast>) {
        for binding in &generic_for.names {
            if let Some(annotation) = &binding.type_annotation {
                self.visit_type(annotation);
            }
        }
        for expr in &generic_for.exprs {
            self.visit_expr(expr);
        }
        self.visit_block(&generic_for.block);
    }

    // Lua 5.5
    fn visit_global_declaration(&mut self, global_declaration: &'ast GlobalDeclaration<'ast>) {
        for expr in &global_declaration.exprs {
            self.visit_expr(expr);
        }
    }

    // Lua 5.5
    fn visit_global_function(&mut self, global_function: &'ast GlobalFunction<'ast>) {
        self.visit_function_body(&global_function.body);
    }

    // Lua 5.5
    fn visit_global_star(&mut self, _global_star: &'ast GlobalStar<'ast>) {}

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if !matches!(expr, Expression::TableConstructor(_) | Expression::Suffixed(_)) {
            self.push_node(NodeKind::Expression(expr), NodeType::of_expr(expr));
        }
        if !matches!(expr, Expression::Name(_)) {
            self.walk_expr(expr);
        }
    }

    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        self.chain(suffixed_expr, false);
        self.walk_suffixed_expr(suffixed_expr);
    }

    fn visit_table_constructor(&mut self, table_constructor: &'ast TableConstructor<'ast>) {
        self.push_node(NodeKind::Table(table_constructor), NodeType::TableConstructor);
        self.walk_table_constructor(table_constructor);
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_token::LuaVersion;

    use super::*;

    fn collect<'ast>(chunk: &'ast Chunk<'ast>) -> Nodes<'ast> {
        Nodes::collect(chunk)
    }

    #[test]
    fn table_is_in_evaluation_order() {
        let allocator = Allocator::new();
        let parse = luck_parser::parse(
            &allocator,
            b"\
local a = 1 + 2
print(a)
t.x = a",
            LuaVersion::Lua54,
        );
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let nodes = collect(&parse.chunk);
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
        let parse = luck_parser::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let nodes = collect(&parse.chunk);
        assert_eq!(nodes.blocks().first().map(|first| first.span), Some(parse.chunk.block.span));
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
