use core::iter;

use luck_ast::{AstKind, GetSpan};
use luck_token::Span;

use crate::id::{NodeId, ScopeId};

/// A node of the tree and the scope it's in, which for a node that opens a
/// scope is the scope around it.
#[derive(Debug, Clone, Copy)]
pub struct AstNode<'ast> {
    kind: AstKind<'ast>,
    scope: ScopeId,
}

impl<'ast> AstNode<'ast> {
    #[must_use]
    pub const fn kind(&self) -> AstKind<'ast> {
        self.kind
    }

    #[must_use]
    pub const fn scope(&self) -> ScopeId {
        self.scope
    }
}

impl GetSpan for AstNode<'_> {
    fn span(&self) -> Span {
        self.kind.span()
    }
}

/// Every node of a tree that has an [`AstKind`], flattened in source
/// preorder, with the id of each one's parent.
///
/// A node comes before what it holds, and what it holds is in source order,
/// so nodes are sorted by where they start, and the nodes inside one follow
/// it.
#[derive(Debug, Default)]
pub struct AstNodes<'ast> {
    nodes: Vec<AstNode<'ast>>,
    /// `None` for the chunk.
    parent_ids: Vec<Option<NodeId>>,
}

impl<'ast> AstNodes<'ast> {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self { nodes: Vec::with_capacity(capacity), parent_ids: Vec::with_capacity(capacity) }
    }

    pub(crate) fn push(&mut self, kind: AstKind<'ast>, scope: ScopeId, parent: Option<NodeId>) {
        self.nodes.push(AstNode { kind, scope });
        self.parent_ids.push(parent);
    }

    /// Zero when the build didn't ask for the table.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every node with its id, in preorder.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (NodeId, &AstNode<'ast>)> + '_ {
        self.nodes.iter().enumerate().map(|(i, node)| (NodeId::from_index(i), node))
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this table, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn get(&self, id: NodeId) -> &AstNode<'ast> {
        &self.nodes[id.index()]
    }

    #[must_use]
    pub fn kind(&self, id: NodeId) -> AstKind<'ast> {
        self.get(id).kind
    }

    /// `None` for the chunk.
    #[must_use]
    pub fn parent_id(&self, id: NodeId) -> Option<NodeId> {
        self.parent_ids.get(id.index()).copied().flatten()
    }

    #[must_use]
    pub fn parent_kind(&self, id: NodeId) -> Option<AstKind<'ast>> {
        self.parent_id(id).map(|parent| self.kind(parent))
    }

    /// Each node `id` is in, its parent first and the chunk last.
    pub fn ancestor_ids(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        iter::successors(self.parent_id(id), |&ancestor| self.parent_id(ancestor))
    }

    pub fn ancestor_kinds(&self, id: NodeId) -> impl Iterator<Item = AstKind<'ast>> + '_ {
        self.ancestor_ids(id).map(|ancestor| self.kind(ancestor))
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
