use luck_token::{CompactString, LuaVersion, Span};
use rustc_hash::{FxHashMap, FxHashSet};

/// Defines an index newtype over `NonZeroU32` holding `index + 1`, so
/// `Option<Id>` stays 4 bytes.
macro_rules! define_index_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(core::num::NonZeroU32);

        impl $name {
            #[must_use]
            pub(crate) fn from_index(index: usize) -> Self {
                let raw = u32::try_from(index.saturating_add(1)).expect("id space overflows u32");
                Self(core::num::NonZeroU32::new(raw).expect("index + 1 is nonzero"))
            }

            #[inline]
            #[must_use]
            pub(crate) fn index(self) -> usize {
                luck_token::offset_to_index(self.0.get().wrapping_sub(1))
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.index())
            }
        }
    };
}

define_index_id!(ScopeId);
define_index_id!(SymbolId);
define_index_id!(ReferenceId);

#[derive(Debug)]
pub struct ScopeTree {
    pub version: LuaVersion,
    pub scopes: Vec<Scope>,
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    /// A span holds at most one reference.
    pub references_by_span: FxHashMap<Span, ReferenceId>,
    /// Lua 5.5: names a `global` statement declares, which are defined
    /// wherever they're read.
    pub declared_globals: FxHashSet<CompactString>,
    /// Globals this chunk writes as free names, so the host's value of each
    /// can't be assumed anywhere in the chunk.
    pub assigned_globals: FxHashSet<CompactString>,
    /// Lua 5.2+: the chunk assigns its own `_ENV`, which redirects every
    /// free name. A local `_ENV` is [`Reference::env`] instead.
    pub is_env_assigned: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Module,
    /// Function body, which introduces an upvalue boundary.
    Function,
    /// `do` block, or an `if`, `elseif`, or `else` body.
    Block,
    /// `while`, `repeat`, or `for` body.
    Loop,
}

#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent facts about a binding, in any combination"
)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: CompactString,
    pub scope: ScopeId,
    pub kind: SymbolKind,
    pub definition_span: Span,
    pub reference_ids: Vec<ReferenceId>,
    pub is_upvalue: bool,
    pub is_modified: bool,
    /// Luau: true for `export local`, `export const`, and `export function`
    /// bindings. The module's export table reads them, so they're used even
    /// with no references in the module, and their name is the module's API.
    pub is_exported: bool,
    /// Lua 5.4+: true for a `<close>` local, which its scope's exit uses even
    /// with no references.
    pub is_close: bool,
    pub shadows: Option<SymbolId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Local,
    Parameter,
    IteratorVariable,
    NumericForVariable,
    FunctionName,
    /// The `self` of a `function t:m()` header. No token spells it, so its
    /// definition span is empty, at the start of the signature.
    ImplicitSelf,
}

#[derive(Debug)]
pub struct Reference {
    pub span: Span,
    pub name: CompactString,
    pub kind: ReferenceKind,
    pub scope: ScopeId,
    /// The symbol this name resolves to, or `None` for a free name.
    pub resolved: Option<SymbolId>,
    /// Lua 5.2+: the local `_ENV` a free name is a field of, in which case
    /// it's no global.
    pub env: Option<SymbolId>,
    /// Lua 5.5: a free name in the scope of a `global` statement that covers
    /// it, by name or with `*`.
    pub is_declared_global: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    Read,
    Write,
    ReadWrite,
}

impl ReferenceKind {
    #[must_use]
    pub const fn is_read(self) -> bool {
        match self {
            Self::Read | Self::ReadWrite => true,
            Self::Write => false,
        }
    }

    #[must_use]
    pub const fn is_write(self) -> bool {
        match self {
            Self::Write | Self::ReadWrite => true,
            Self::Read => false,
        }
    }
}

impl ScopeTree {
    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this arena, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.index()]
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this arena, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.index()]
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this arena, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn reference(&self, id: ReferenceId) -> &Reference {
        &self.references[id.index()]
    }

    pub(crate) fn reference_at(&self, name: &str, span: Span) -> Option<&Reference> {
        let reference = self.reference(*self.references_by_span.get(&span)?);
        (reference.name == name).then_some(reference)
    }

    /// The local that the identifier `name` at `span` resolves to: `None`
    /// for a free name, and for a span that holds no reference.
    pub(crate) fn resolved_symbol(&self, name: &str, span: Span) -> Option<SymbolId> {
        self.reference_at(name, span)?.resolved
    }

    #[must_use]
    pub fn resolves_to_local(&self, name: &str, span: Span) -> bool {
        self.resolved_symbol(name, span).is_some()
    }

    /// True when the identifier `name` at `span` reads the host's global of
    /// that name, so a stdlib fact about `name` holds there.
    ///
    /// False for a local, a field of a local `_ENV`, a chunk that assigns its
    /// own `_ENV`, a global this chunk writes anywhere, and a span that holds
    /// no reference.
    #[must_use]
    pub fn reaches_host_global(&self, name: &str, span: Span) -> bool {
        self.reference_at(name, span).is_some_and(|reference| self.is_host_global(reference))
    }

    /// See [`Self::reaches_host_global`].
    pub(crate) fn is_host_global(&self, reference: &Reference) -> bool {
        self.is_global(reference) && !self.assigned_globals.contains(&reference.name)
    }

    /// Whether `symbol` is a Lua 5.2+ `_ENV`, which every free name in its
    /// scope indexes.
    #[must_use]
    pub fn is_env_binding(&self, symbol: SymbolId) -> bool {
        self.version.has_env_upvalue() && self.symbol(symbol).name == "_ENV"
    }

    const fn is_global(&self, reference: &Reference) -> bool {
        reference.resolved.is_none() && reference.env.is_none() && !self.is_env_assigned
    }

    /// Every reference to a global. A free name that indexes a `_ENV` the
    /// chunk controls isn't one.
    pub fn unresolved_references(&self) -> impl Iterator<Item = &Reference> {
        self.references.iter().filter(|reference| self.is_global(reference))
    }

    /// Whether code in the scope `from` runs in a function nested inside the
    /// scope `to`, rather than in the function that holds `to`.
    #[must_use]
    pub fn crosses_function_boundary(&self, from: ScopeId, to: ScopeId) -> bool {
        crosses_function_boundary(&self.scopes, from, to)
    }
}

pub(crate) fn crosses_function_boundary(scopes: &[Scope], from: ScopeId, to: ScopeId) -> bool {
    let mut current = Some(from);
    while let Some(scope_id) = current {
        if scope_id == to {
            return false;
        }
        let Some(scope) = scopes.get(scope_id.index()) else {
            return false;
        };
        if scope.kind == ScopeKind::Function {
            return true;
        }
        current = scope.parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_niche_optimized() {
        assert_eq!(size_of::<Option<ScopeId>>(), 4);
        assert_eq!(size_of::<Option<SymbolId>>(), 4);
        assert_eq!(size_of::<Option<ReferenceId>>(), 4);
    }

    #[test]
    fn id_roundtrips_index() {
        let id = SymbolId::from_index(7);
        assert_eq!(id.index(), 7);
        assert_eq!(format!("{id:?}"), "SymbolId(7)");
    }
}
