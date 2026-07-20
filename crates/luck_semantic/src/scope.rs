use compact_str::CompactString;
use luck_token::Span;

/// Defines a typed scope-tree index over `NonZeroU32` storing `index + 1`,
/// so `Option<Id>` stays 4 bytes via the niche.
macro_rules! define_index_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(std::num::NonZeroU32);

        impl $name {
            #[must_use]
            pub fn from_index(index: usize) -> Self {
                let raw = u32::try_from(index + 1).expect("id space overflows u32");
                Self(std::num::NonZeroU32::new(raw).expect("index + 1 is nonzero"))
            }

            #[must_use]
            pub fn index(self) -> usize {
                self.0.get() as usize - 1
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, concat!(stringify!($name), "({})"), self.index())
            }
        }
    };
}

pub(crate) use define_index_id;

define_index_id!(ScopeId);
define_index_id!(SymbolId);
define_index_id!(ReferenceId);

/// The complete scope tree for a Lua chunk.
#[derive(Debug)]
pub struct ScopeTree {
    pub scopes: Vec<Scope>,
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
}

/// A lexical scope in the program.
#[derive(Debug)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    pub span: Span,
    pub symbols: Vec<SymbolId>,
    pub children: Vec<ScopeId>,
}

/// What kind of scope this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// Top-level block of a file.
    Module,
    /// Function body - introduces an upvalue boundary.
    Function,
    /// do...end, if body, else body.
    Block,
    /// while/repeat/for - controls break/continue semantics.
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
    /// All references (reads/writes) to this symbol.
    pub reference_ids: Vec<ReferenceId>,
    /// True if this symbol is captured by a nested function.
    pub is_upvalue: bool,
    /// True if this symbol is assigned to after its initial definition.
    pub is_modified: bool,
    /// The symbol this one shadows, if any.
    pub shadows: Option<SymbolId>,
}

/// What kind of symbol declaration this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Local,
    Parameter,
    IteratorVariable,
    NumericForVariable,
    FunctionName,
}

/// A reference to a name in the source code.
#[derive(Debug)]
pub struct Reference {
    pub id: ReferenceId,
    pub span: Span,
    pub name: CompactString,
    pub kind: ReferenceKind,
    /// The scope this reference occurs in.
    pub scope: ScopeId,
    /// The resolved symbol, if any. None means it's a global.
    pub resolved: Option<SymbolId>,
}

/// Whether a reference reads or writes the variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    Read,
    Write,
    ReadWrite,
}

impl ScopeTree {
    pub(crate) fn new() -> Self {
        Self {
            scopes: Vec::new(),
            symbols: Vec::new(),
            references: Vec::new(),
        }
    }

    #[must_use]
    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.index()]
    }

    #[must_use]
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.index()]
    }

    #[must_use]
    pub fn reference(&self, id: ReferenceId) -> &Reference {
        &self.references[id.index()]
    }

    /// The reference the builder recorded at `span` for `name`, if any.
    /// Spans are unique source locations, so at most one matches. Used by
    /// the shape/local-resolution queries that key off a source slice.
    #[must_use]
    pub fn reference_at(&self, name: &str, span: Span) -> Option<&Reference> {
        self.references
            .iter()
            .find(|reference| reference.span == span && reference.name == name)
    }

    pub(crate) fn add_scope(
        &mut self,
        parent: Option<ScopeId>,
        kind: ScopeKind,
        span: Span,
    ) -> ScopeId {
        let id = ScopeId::from_index(self.scopes.len());
        self.scopes.push(Scope {
            id,
            parent,
            kind,
            span,
            symbols: Vec::new(),
            children: Vec::new(),
        });
        if let Some(parent_id) = parent {
            self.scopes[parent_id.index()].children.push(id);
        }
        id
    }

    /// Record a declared symbol. Resolution happens at build time on the
    /// builder's flat binding stack, so `shadows` arrives pre-computed.
    pub(crate) fn add_symbol(
        &mut self,
        name: CompactString,
        scope: ScopeId,
        kind: SymbolKind,
        definition_span: Span,
        shadows: Option<SymbolId>,
    ) -> SymbolId {
        let id = SymbolId::from_index(self.symbols.len());
        self.symbols.push(Symbol {
            id,
            name,
            scope,
            kind,
            definition_span,
            reference_ids: Vec::new(),
            is_upvalue: false,
            is_modified: false,
            shadows,
        });
        self.scopes[scope.index()].symbols.push(id);
        id
    }

    /// Record a reference whose binding was already resolved on the
    /// builder's binding stack (`None` = global).
    pub(crate) fn add_reference(
        &mut self,
        name: CompactString,
        span: Span,
        scope: ScopeId,
        kind: ReferenceKind,
        resolved: Option<SymbolId>,
    ) -> ReferenceId {
        let id = ReferenceId::from_index(self.references.len());

        if let Some(sym_id) = resolved {
            let sym_scope = self.symbols[sym_id.index()].scope;
            if self.crosses_function_boundary(scope, sym_scope) {
                self.symbols[sym_id.index()].is_upvalue = true;
            }
            self.symbols[sym_id.index()].reference_ids.push(id);
            if matches!(kind, ReferenceKind::Write | ReferenceKind::ReadWrite) {
                self.symbols[sym_id.index()].is_modified = true;
            }
        }

        self.references.push(Reference {
            id,
            span,
            name,
            kind,
            scope,
            resolved,
        });
        id
    }

    /// Check if traversing from `from` scope to `to` scope crosses a function boundary.
    fn crosses_function_boundary(&self, from: ScopeId, to: ScopeId) -> bool {
        let mut current = Some(from);
        while let Some(scope_id) = current {
            if scope_id == to {
                return false;
            }
            if self.scopes[scope_id.index()].kind == ScopeKind::Function {
                return true;
            }
            current = self.scopes[scope_id.index()].parent;
        }
        false
    }

    /// Get all unresolved references (globals).
    pub fn unresolved_references(&self) -> impl Iterator<Item = &Reference> {
        self.references.iter().filter(|r| r.resolved.is_none())
    }

    /// Get all symbols that have zero read references.
    pub fn unused_symbols(&self) -> impl Iterator<Item = &Symbol> {
        self.symbols.iter().filter(|sym| {
            !sym.reference_ids.iter().any(|&ref_id| {
                matches!(
                    self.references[ref_id.index()].kind,
                    ReferenceKind::Read | ReferenceKind::ReadWrite
                )
            })
        })
    }
}

impl Default for ScopeTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_niche_optimized() {
        assert_eq!(std::mem::size_of::<Option<ScopeId>>(), 4);
        assert_eq!(std::mem::size_of::<Option<SymbolId>>(), 4);
        assert_eq!(std::mem::size_of::<Option<ReferenceId>>(), 4);
    }

    #[test]
    fn id_roundtrips_index() {
        let id = SymbolId::from_index(7);
        assert_eq!(id.index(), 7);
        assert_eq!(format!("{id:?}"), "SymbolId(7)");
    }
}
