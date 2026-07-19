//! Flat table of every `Statement` and `Expression` node, in the same
//! pre-order the linter's shared walk used to produce. Rules iterate this
//! once instead of re-walking the tree; each entry carries its parent and
//! enclosing scope.

use luck_ast::node::{AstTypesBitset, NodeKind, NodeType};
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;

use crate::scope::{ScopeId, ScopeTree, define_index_id};

define_index_id!(NodeId);

/// One AST node plus its table links.
pub struct AstNode<'ast> {
    pub id: NodeId,
    pub kind: NodeKind<'ast>,
    pub node_type: NodeType,
    pub parent: Option<NodeId>,
    pub scope: ScopeId,
}

/// The flat node table for one chunk.
pub struct Nodes<'ast> {
    nodes: Vec<AstNode<'ast>>,
    present_types: AstTypesBitset,
}

impl<'ast> Nodes<'ast> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[must_use]
    pub fn node(&self, id: NodeId) -> &AstNode<'ast> {
        &self.nodes[id.index()]
    }

    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<&AstNode<'ast>> {
        self.node(id).parent.map(|parent_id| self.node(parent_id))
    }

    pub fn iter(&self) -> impl Iterator<Item = &AstNode<'ast>> {
        self.nodes.iter()
    }

    pub fn as_slice(&self) -> &[AstNode<'ast>] {
        &self.nodes
    }

    /// Whether the chunk contains any node whose type is in `types`.
    #[must_use]
    pub fn contains_any(&self, types: &AstTypesBitset) -> bool {
        self.present_types.intersects(types)
    }
}

/// Collect the node table for `block`. Traversal order matches the
/// default [`Visitor`] walk exactly; the scope builder's walk visits in
/// resolution order instead, so the table is built by its own pass.
#[must_use]
pub fn collect_nodes<'ast>(block: &'ast Block, scope_tree: &ScopeTree) -> Nodes<'ast> {
    let mut collector = NodeCollector {
        nodes: Vec::new(),
        present_types: AstTypesBitset::new(),
        current_parent: None,
        sweep: ScopeSweep::new(scope_tree),
        _marker: std::marker::PhantomData,
    };
    collector.visit_block(block);
    Nodes {
        nodes: collector.nodes,
        present_types: collector.present_types,
    }
}

/// Assigns each node its innermost enclosing scope by sweeping the scope
/// list in span order. Node visits arrive with non-decreasing span
/// starts (the pre-order walk emits source order), so one forward pass
/// with a stack suffices.
struct ScopeSweep {
    /// (span start, span end, id), sorted by start ascending then end
    /// descending so outer scopes are entered before inner ones.
    ordered: Vec<(u32, u32, ScopeId)>,
    next: usize,
    /// (id, span end); the module scope at the bottom is never popped.
    stack: Vec<(ScopeId, u32)>,
}

impl ScopeSweep {
    fn new(scope_tree: &ScopeTree) -> Self {
        let module = &scope_tree.scopes[0];
        let mut ordered: Vec<(u32, u32, ScopeId)> = scope_tree.scopes[1..]
            .iter()
            .map(|scope| (scope.span.start, scope.span.end, scope.id))
            .collect();
        ordered.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        Self {
            ordered,
            next: 0,
            stack: vec![(module.id, module.span.end)],
        }
    }

    fn scope_at(&mut self, position: u32) -> ScopeId {
        while self.next < self.ordered.len() && self.ordered[self.next].0 <= position {
            let (_, end, id) = self.ordered[self.next];
            self.next += 1;
            self.pop_ended(position);
            self.stack.push((id, end));
        }
        self.pop_ended(position);
        self.stack.last().expect("module scope never pops").0
    }

    fn pop_ended(&mut self, position: u32) {
        while self.stack.len() > 1 && self.stack.last().expect("len checked").1 <= position {
            self.stack.pop();
        }
    }
}

struct NodeCollector<'ast> {
    nodes: Vec<AstNode<'ast>>,
    present_types: AstTypesBitset,
    current_parent: Option<NodeId>,
    sweep: ScopeSweep,
    _marker: std::marker::PhantomData<&'ast Block>,
}

impl<'ast> NodeCollector<'ast> {
    fn push_node(&mut self, kind: NodeKind<'ast>, span_start: u32) -> NodeId {
        let id = NodeId::from_index(self.nodes.len());
        let node_type = kind.node_type();
        self.present_types.set(node_type);
        self.nodes.push(AstNode {
            id,
            kind,
            node_type,
            parent: self.current_parent,
            scope: self.sweep.scope_at(span_start),
        });
        id
    }
}

/// Restores the `'ast` lifetime the `Visitor` trait's anonymous-lifetime
/// signatures erase.
///
/// SAFETY: `NodeCollector` is private and only driven by `collect_nodes`,
/// which starts the walk at the one `&'ast Block` it was given; every ref
/// the default `walk_*` methods pass back down is a re-borrow of that
/// block's subtree and therefore lives for `'ast`.
unsafe fn restore_ast_lifetime<'ast, T: ?Sized>(reference: &T) -> &'ast T {
    unsafe { &*std::ptr::from_ref(reference) }
}

impl<'ast> Visitor for NodeCollector<'ast> {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        // SAFETY: see `restore_ast_lifetime`.
        let stmt: &'ast luck_ast::Statement = unsafe { restore_ast_lifetime(stmt) };
        let saved_parent = self.current_parent;
        let id = self.push_node(NodeKind::Statement(stmt), stmt.span().start);
        self.current_parent = Some(id);
        self.walk_statement(stmt);
        self.current_parent = saved_parent;
    }

    fn visit_expression(&mut self, expr: &luck_ast::Expression) {
        // SAFETY: see `restore_ast_lifetime`.
        let expr: &'ast luck_ast::Expression = unsafe { restore_ast_lifetime(expr) };
        let saved_parent = self.current_parent;
        let id = self.push_node(NodeKind::Expression(expr), expr.span().start);
        self.current_parent = Some(id);
        self.walk_expression(expr);
        self.current_parent = saved_parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn collect(source: &str) -> (luck_parser::ParseResult, ScopeTree) {
        let parse = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let tree = crate::builder::ScopeTreeBuilder::new().build(&parse.block);
        (parse, tree)
    }

    #[test]
    fn table_matches_visitor_preorder() {
        let (parse, tree) = collect("local a = 1 + 2\nprint(a)");
        let nodes = collect_nodes(&parse.block, &tree);
        let types: Vec<NodeType> = nodes.iter().map(|node| node.node_type).collect();
        assert_eq!(
            types,
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
        assert!(nodes.contains_any(&AstTypesBitset::from_types(&[NodeType::BinaryOp])));
        assert!(!nodes.contains_any(&AstTypesBitset::from_types(&[NodeType::WhileLoop])));
    }

    #[test]
    fn parents_link_to_enclosing_node() {
        let (parse, tree) = collect("local a = 1 + 2");
        let nodes = collect_nodes(&parse.block, &tree);
        let binop = nodes
            .iter()
            .find(|node| node.node_type == NodeType::BinaryOp)
            .unwrap();
        let parent = nodes.parent(binop.id).unwrap();
        assert_eq!(parent.node_type, NodeType::LocalAssignment);
        assert!(nodes.node(binop.id).parent.is_some());
        let number = nodes
            .iter()
            .find(|node| node.node_type == NodeType::Number)
            .unwrap();
        assert_eq!(
            nodes.parent(number.id).unwrap().node_type,
            NodeType::BinaryOp
        );
    }

    #[test]
    fn scope_attribution_tracks_nesting() {
        let (parse, tree) = collect("local a = 1\ndo\n  local b = 2\nend\nlocal c = 3");
        let nodes = collect_nodes(&parse.block, &tree);
        let statements: Vec<&AstNode> = nodes
            .iter()
            .filter(|node| node.node_type == NodeType::LocalAssignment)
            .collect();
        assert_eq!(statements.len(), 3, "a, b, c");
        let module_scope = statements[0].scope;
        assert_eq!(
            statements[2].scope, module_scope,
            "c is back at module scope"
        );
        assert_ne!(
            statements[1].scope, module_scope,
            "b is inside the do-block"
        );
    }
}
