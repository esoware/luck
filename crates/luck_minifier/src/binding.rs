//! The scoping a phase reads its bindings from, and what a pass reads of a
//! binding beyond it: how its declaration fixes it, and how `luck_value`
//! tells a local's read from a global's.

use luck_ast::{
    Chunk, LocalAssignment, LocalAssignmentKind, LocalFunction, Name, NameIndex, TreeCounts,
};
use luck_semantic::{Scoping, SemanticBuilder};
use luck_token::LuaVersion;
use luck_value::ValueContext;

pub(crate) const METHOD_RECEIVER_NAME: &str = "self";

pub(crate) const IMPLICIT_VARARG_TABLE_NAME: &str = "arg";

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
    fn version(&self) -> LuaVersion {
        self.0.version()
    }

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

/// Whether `name` declares the `_ENV` upvalue every global read and write
/// goes through. Outside 5.2+ the name binds an ordinary local, so pinning
/// it would cost bytes for nothing.
pub(crate) fn declares_env(name: &Name<'_>, scoping: &Scoping) -> bool {
    scoping.symbol_of(name).is_some_and(|symbol| scoping.is_env_binding(symbol))
}

/// Whether a `local` declaration binds a name with an attribute (`<close>`
/// runs `__close` at scope exit and `<const>` affects validity) or a 5.2+
/// `_ENV`, which redirects global lookup for everything after it.
pub(crate) fn has_attributed_or_env_name(local: &LocalAssignment<'_>, scoping: &Scoping) -> bool {
    local.attribute.is_some()
        || local.names.iter().any(|attributed| {
            attributed.attribute.is_some() || declares_env(&attributed.name, scoping)
        })
}

/// Whether no transform may reshape, merge away, or move a `local`
/// declaration: an export, a `const`, or one that
/// [`has_attributed_or_env_name`].
pub(crate) fn has_fixed_binding(local: &LocalAssignment<'_>, scoping: &Scoping) -> bool {
    local.kind != LocalAssignmentKind::Local || has_attributed_or_env_name(local, scoping)
}

pub(crate) fn is_droppable_function(local_function: &LocalFunction<'_>, scoping: &Scoping) -> bool {
    !local_function.kind.is_exported() && !declares_env(&local_function.name, scoping)
}
