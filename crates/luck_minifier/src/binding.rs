//! What a pass reads of a binding beyond the scoping: how its declaration
//! fixes it, and how `luck_value` tells a local's read from a global's.

use luck_ast::{LocalAssignment, LocalAssignmentKind, LocalFunction, Name, NameIndex};
use luck_semantic::Scoping;
use luck_token::LuaVersion;
use luck_value::ValueContext;

pub(crate) const METHOD_RECEIVER_NAME: &str = "self";

pub(crate) const IMPLICIT_VARARG_TABLE_NAME: &str = "arg";

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
        self.0.reference_of(name).is_none_or(|id| self.0.reference(id).resolved.is_none())
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
        self.next = self.next.saturating_add(1);
        index
    }
}

/// Whether this name is the `_ENV` upvalue every global read and write
/// goes through. Outside 5.2+ the name binds an ordinary local, so
/// pinning it would cost bytes for nothing.
pub(crate) fn is_env_binding(name: &str, version: LuaVersion) -> bool {
    version.has_env_upvalue() && name == "_ENV"
}

/// Whether a `local` declaration binds a name with an attribute (`<close>`
/// runs `__close` at scope exit and `<const>` affects validity) or a 5.2+
/// `_ENV`, which redirects global lookup for everything after it.
pub(crate) fn has_attributed_or_env_name(local: &LocalAssignment<'_>, version: LuaVersion) -> bool {
    local.attribute.is_some()
        || local.names.iter().any(|attributed| {
            attributed.attribute.is_some() || is_env_binding(attributed.name.text.as_str(), version)
        })
}

/// Whether no transform may reshape, merge away, or move a `local`
/// declaration: an export, a `const`, or one that
/// [`has_attributed_or_env_name`].
pub(crate) fn has_fixed_binding(local: &LocalAssignment<'_>, version: LuaVersion) -> bool {
    local.kind != LocalAssignmentKind::Local || has_attributed_or_env_name(local, version)
}

pub(crate) fn is_droppable_function(
    local_function: &LocalFunction<'_>,
    version: LuaVersion,
) -> bool {
    !local_function.kind.is_exported()
        && !is_env_binding(local_function.name.text.as_str(), version)
}
