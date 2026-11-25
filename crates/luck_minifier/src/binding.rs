//! The scoping a phase reads its bindings from, and what a pass reads of a
//! binding beyond it: how its declaration fixes it, and how `luck_value`
//! tells a local's read from a global's.

use luck_ast::{Chunk, LocalAssignment, LocalAssignmentKind, Name, NameIndex, TreeCounts};
use luck_semantic::{Scoping, SemanticBuilder};
use luck_value::ValueContext;

pub(crate) const METHOD_RECEIVER_NAME: &str = "self";

/// `chunk`'s scoping, which a rewrite reads its bindings from, in the tables
/// of `reused`, an earlier analysis', when there is one.
pub(crate) fn analyze(chunk: &Chunk<'_>, counts: TreeCounts, reused: Option<Scoping>) -> Scoping {
    let builder = SemanticBuilder::new().with_counts(counts);
    let built = match reused {
        Some(scoping) => builder.rebuild(chunk, scoping),
        None => builder.build(chunk),
    };
    built.semantic.into_scoping()
}

/// The names of a tree as its scoping resolves them, for `luck_value`: a
/// name that resolves to no local reads a global, and so does one the
/// scoping never saw.
#[derive(Clone, Copy)]
pub(crate) struct ResolvedNames<'scoping>(pub &'scoping Scoping);

impl ValueContext for ResolvedNames<'_> {
    fn is_global_reference(&self, name: &Name<'_>) -> bool {
        !self.0.resolves_to_local(name)
    }
}

/// The indexes a pass gives the names it builds, each past every index the
/// tree held, so an analysis of the tree finds each name's role.
pub(crate) struct NameIndexes {
    next: u32,
}

impl NameIndexes {
    /// The indexes from `first`, one past the tree's last.
    pub(crate) const fn new(first: u32) -> Self {
        Self { next: first }
    }

    pub(crate) const fn next(&mut self) -> NameIndex {
        let index = NameIndex::new(self.next);
        self.next += 1;
        index
    }
}

/// Whether no transform may reshape, merge away, or move a `local`
/// declaration: an export or a `const`.
pub(crate) fn has_fixed_binding(local: &LocalAssignment<'_>) -> bool {
    local.kind != LocalAssignmentKind::Local
}
