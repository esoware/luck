use luck_token::{CompactString, Span};
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
            fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(formatter, concat!(stringify!($name), "({})"), self.index())
            }
        }
    };
}

define_index_id!(ScopeId);
define_index_id!(SymbolId);
define_index_id!(ReferenceId);

/// Every scope, symbol, and reference in one chunk, indexed by id.
#[derive(Debug)]
pub struct ScopeTree {
    pub scopes: Vec<Scope>,
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    /// Each name reference by its span. A span holds at most one reference.
    pub references_by_span: FxHashMap<Span, ReferenceId>,
    /// Lua 5.5: names a `global` statement declares, which are defined
    /// wherever they're read.
    pub declared_globals: FxHashSet<CompactString>,
}

#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// Top-level block of a file.
    Module,
    /// Function body, which introduces an upvalue boundary.
    Function,
    /// `do` block, or an `if`, `elseif`, or `else` body.
    Block,
    /// `while`, `repeat`, or `for` body.
    Loop,
}

/// A declared symbol (local variable, parameter, etc.).
#[derive(Debug)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: CompactString,
    pub scope: ScopeId,
    pub kind: SymbolKind,
    pub definition_span: Span,
    pub reference_ids: Vec<ReferenceId>,
    /// True if this symbol is captured by a nested function.
    pub is_upvalue: bool,
    /// True if this symbol is assigned to after its initial definition.
    pub is_modified: bool,
    /// Luau: true for `export local`, `export const`, and `export function`
    /// bindings. The module's export table reads them, so they're used even
    /// with no references in the module.
    pub is_exported: bool,
    pub shadows: Option<SymbolId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Local,
    Parameter,
    IteratorVariable,
    NumericForVariable,
    FunctionName,
}

/// One use of a name, local or global.
#[derive(Debug)]
pub struct Reference {
    pub span: Span,
    pub name: CompactString,
    pub kind: ReferenceKind,
    pub scope: ScopeId,
    /// The symbol this name resolves to, or `None` for a global.
    pub resolved: Option<SymbolId>,
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

    /// The local that the identifier `name` at `span` resolves to: `None`
    /// for a global, and for a span that holds no reference.
    pub(crate) fn resolved_symbol(&self, name: &str, span: Span) -> Option<SymbolId> {
        let reference = self.reference(*self.references_by_span.get(&span)?);
        if reference.name == name { reference.resolved } else { None }
    }

    /// True when the identifier `name` at `span` resolves to a local rather
    /// than a global.
    #[must_use]
    pub fn resolves_to_local(&self, name: &str, span: Span) -> bool {
        self.resolved_symbol(name, span).is_some()
    }

    /// All unresolved references, which are the globals.
    pub fn unresolved_references(&self) -> impl Iterator<Item = &Reference> {
        self.references.iter().filter(|reference| reference.resolved.is_none())
    }
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
