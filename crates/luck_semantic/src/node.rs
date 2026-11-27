use std::iter;

use luck_ast::{AstKind, AstType, GetSpan};
use luck_token::Span;

use crate::ast_types_bitset::AstTypesBitset;
use crate::id::{NodeId, ScopeId};

/// A node of the tree, its id, and the scope it's in, which for a node that
/// opens a scope is the scope around it.
#[derive(Debug, Clone, Copy)]
pub struct AstNode<'ast> {
    kind: AstKind<'ast>,
    scope: ScopeId,
    id: NodeId,
}

impl<'ast> AstNode<'ast> {
    pub const fn kind(&self) -> AstKind<'ast> {
        self.kind
    }

    pub const fn scope(&self) -> ScopeId {
        self.scope
    }

    pub const fn id(&self) -> NodeId {
        self.id
    }
}

impl GetSpan for AstNode<'_> {
    fn span(&self) -> Span {
        self.kind.span()
    }
}

/// Every node of a tree that has an [`AstKind`], flattened in source
/// preorder, with the id of each one's parent, as `oxc_semantic`'s.
///
/// A node comes before what it holds, and what it holds is in source order,
/// so nodes are sorted by where they start, and the nodes inside one follow
/// it.
#[derive(Debug, Default)]
pub struct AstNodes<'ast> {
    nodes: Vec<AstNode<'ast>>,
    /// `None` for the chunk.
    parent_ids: Vec<Option<NodeId>>,
    /// Every type of node the tree holds.
    node_types: AstTypesBitset,
}

impl<'ast> AstNodes<'ast> {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
            parent_ids: Vec::with_capacity(capacity),
            node_types: AstTypesBitset::new(),
        }
    }

    /// Adds the node `id`, the next in preorder.
    pub(crate) fn push(
        &mut self,
        id: NodeId,
        kind: AstKind<'ast>,
        scope: ScopeId,
        parent: Option<NodeId>,
    ) {
        debug_assert_eq!(id.index(), self.nodes.len(), "nodes are added in id order");
        self.nodes.push(AstNode { kind, scope, id });
        self.parent_ids.push(parent);
        self.node_types.set(kind.ty());
    }

    /// Zero when the build didn't ask for the table.
    pub const fn len(&self) -> usize {
        self.nodes.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every node, in preorder.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &AstNode<'ast>> + '_ {
        self.nodes.iter()
    }

    pub fn get(&self, id: NodeId) -> &AstNode<'ast> {
        &self.nodes[id.index()]
    }

    pub fn kind(&self, id: NodeId) -> AstKind<'ast> {
        self.get(id).kind
    }

    /// `None` for the chunk.
    pub fn parent_id(&self, id: NodeId) -> Option<NodeId> {
        self.parent_ids[id.index()]
    }

    /// `None` for the chunk.
    pub fn parent_node(&self, id: NodeId) -> Option<&AstNode<'ast>> {
        self.parent_id(id).map(|parent| self.get(parent))
    }

    /// `None` for the chunk.
    pub fn parent_kind(&self, id: NodeId) -> Option<AstKind<'ast>> {
        self.parent_id(id).map(|parent| self.kind(parent))
    }

    /// Each node `id` is in, its parent first and the chunk last.
    pub fn ancestor_ids(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        iter::successors(self.parent_id(id), |&ancestor| self.parent_id(ancestor))
    }

    /// The innermost node whose span holds `offset`, either end included, so
    /// an editor's cursor just past a name is on it, and of two that meet
    /// there, the later. `None` outside every node.
    ///
    /// The last node to start by `offset` is that node or inside it, since
    /// nodes are sorted by where they start and the nodes inside one follow
    /// it, so the search is one binary search and a walk up from there.
    pub fn node_at(&self, offset: u32) -> Option<NodeId> {
        let after = self.nodes.partition_point(|node| node.span().start <= offset);
        let last = NodeId::from_index(after.checked_sub(1)?);
        iter::once(last)
            .chain(self.ancestor_ids(last))
            .find(|&id| offset <= self.get(id).span().end)
    }

    /// Whether the tree holds a node of `ty`.
    pub const fn contains(&self, ty: AstType) -> bool {
        self.node_types.has(ty)
    }

    /// Whether the tree holds a node of any type in `types`.
    pub fn contains_any(&self, types: AstTypesBitset) -> bool {
        self.node_types.intersects(types)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn a_row_is_three_words_and_a_parent() {
        assert_eq!(size_of::<AstNode<'_>>(), 24);
        assert_eq!(size_of::<Option<NodeId>>(), 4);
    }
}
