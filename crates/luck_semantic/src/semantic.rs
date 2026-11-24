use luck_token::LuaVersion;

use crate::node::AstNodes;
use crate::require_site::RequireSite;
use crate::scoping::Scoping;

/// What [`SemanticBuilder`](crate::SemanticBuilder) found in one chunk: its
/// [`Scoping`], always, and its node table and `require` sites, when the
/// build asked for them.
#[derive(Debug)]
pub struct Semantic<'ast> {
    scoping: Scoping,
    nodes: AstNodes<'ast>,
    requires: Vec<RequireSite<'ast>>,
}

impl<'ast> Semantic<'ast> {
    pub(crate) const fn new(
        scoping: Scoping,
        nodes: AstNodes<'ast>,
        requires: Vec<RequireSite<'ast>>,
    ) -> Self {
        Self { scoping, nodes, requires }
    }

    pub const fn version(&self) -> LuaVersion {
        self.scoping.version()
    }

    pub const fn scoping(&self) -> &Scoping {
        &self.scoping
    }

    /// Empty unless [`SemanticBuilder::with_nodes`](crate::SemanticBuilder::with_nodes)
    /// asked for it.
    pub const fn nodes(&self) -> &AstNodes<'ast> {
        &self.nodes
    }

    /// In source order. Empty unless
    /// [`SemanticBuilder::with_requires`](crate::SemanticBuilder::with_requires)
    /// asked for them.
    pub fn requires(&self) -> &[RequireSite<'ast>] {
        &self.requires
    }

    /// The scoping alone, which borrows nothing from the tree, so a pass can
    /// keep it while it rewrites the tree.
    pub fn into_scoping(self) -> Scoping {
        self.scoping
    }
}
