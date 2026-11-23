use luck_ast::{Call, StringLiteral};
use luck_token::LuaVersion;

use crate::node::AstNodes;
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

    #[must_use]
    pub const fn version(&self) -> LuaVersion {
        self.scoping.version()
    }

    #[must_use]
    pub const fn scoping(&self) -> &Scoping {
        &self.scoping
    }

    /// Empty unless [`SemanticBuilder::with_nodes`](crate::SemanticBuilder::with_nodes)
    /// asked for it.
    #[must_use]
    pub const fn nodes(&self) -> &AstNodes<'ast> {
        &self.nodes
    }

    /// In source order. Empty unless
    /// [`SemanticBuilder::with_requires`](crate::SemanticBuilder::with_requires)
    /// asked for them.
    #[must_use]
    pub fn requires(&self) -> &[RequireSite<'ast>] {
        &self.requires
    }

    /// The scoping alone, which borrows nothing from the tree, so a pass can
    /// keep it while it rewrites the tree.
    #[must_use]
    pub fn into_scoping(self) -> Scoping {
        self.scoping
    }
}

/// A call of the host's global `require`, as `require(path)` or
/// `require "path"`, which loads a module.
///
/// A method call, a call of a local or of a global the chunk writes, and a
/// call under a local `_ENV` or one the chunk assigns load nothing the chunk
/// can know of, so they're no site.
#[derive(Debug, Clone, Copy)]
pub struct RequireSite<'ast> {
    pub call: Call<'ast>,
    /// The module path, when the call passes one string literal and nothing
    /// else, and `None` for a path only the running program knows.
    pub path: Option<&'ast StringLiteral<'ast>>,
}
