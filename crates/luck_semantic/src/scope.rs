mod builder;

use luck_ast::{Block, Name, TreeCounts};
use luck_token::{LuaVersion, Span, offset_to_index};
use rustc_hash::FxHashMap;

use crate::sink::NodeSink;
pub(crate) use builder::build_with_initializers;

/// Defines an index newtype over `NonZeroU32` holding `index + 1`, so
/// `Option<Id>` stays 4 bytes.
macro_rules! define_index_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(core::num::NonZeroU32);

        impl $name {
            /// A table never holds more rows than its chunk has bytes, so an
            /// index always fits.
            pub(crate) fn from_index(index: usize) -> Self {
                let offset = u32::try_from(index).unwrap_or(u32::MAX);
                Self(core::num::NonZeroU32::MIN.saturating_add(offset))
            }

            /// Its row in its table, for a consumer's side table.
            #[inline]
            #[must_use]
            pub fn index(self) -> usize {
                offset_to_index(self.0.get().wrapping_sub(1))
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
define_index_id!(NameId);

/// The scopes, symbols, and references of one chunk, in flat tables indexed
/// by their ids, and the distinct names they use.
///
/// Symbols are in the order the walk declares them, and references in the
/// order the chunk evaluates them, which is source order except that an
/// assignment's values come before its targets.
#[derive(Debug)]
pub struct ScopeTree<'ast> {
    version: LuaVersion,
    scopes: Vec<Scope>,
    symbols: Vec<Symbol>,
    references: Vec<Reference>,
    /// Every symbol's references, a run per symbol in symbol order.
    resolved: Vec<ReferenceId>,
    /// Where each symbol's run of `resolved` starts, and then where the last
    /// one ends.
    resolved_offsets: Vec<u32>,
    /// Every reference by where it starts, sorted.
    positions: Vec<Position>,
    /// For each bucket of source bytes, where its run of `positions` starts,
    /// and then where the last one ends. Empty when the references are too
    /// sparse for it to pay, and then a lookup searches them all.
    buckets: Vec<u32>,
    names: Vec<NameEntry<'ast>>,
    name_ids: FxHashMap<&'ast str, NameId>,
    is_env_assigned: bool,
}

/// A bucket is 32 bytes, so it holds at most 16 references, since each
/// takes a byte and something must separate it from the next.
const BUCKET_SHIFT: u32 = 5;

/// Past this many buckets per reference, the buckets cost more memory than
/// the search they save is worth.
const MAX_BUCKETS_PER_REFERENCE: usize = 4;

#[derive(Debug, Clone, Copy)]
struct Position {
    start: u32,
    reference: ReferenceId,
}

#[derive(Debug, Clone, Copy)]
struct NameEntry<'ast> {
    text: &'ast str,
    is_assigned_global: bool,
    is_declared_global: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    /// The function scope this one runs in: itself for a function or the
    /// chunk.
    pub function: ScopeId,
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

#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent facts about a binding, in any combination"
)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: NameId,
    pub scope: ScopeId,
    pub kind: SymbolKind,
    pub definition_span: Span,
    pub shadows: Option<SymbolId>,
    pub is_upvalue: bool,
    pub is_modified: bool,
    /// Luau: true for `export local`, `export const`, and `export function`
    /// bindings. The module's export table reads them, so they're used even
    /// with no references in the module, and their name is the module's API.
    pub is_exported: bool,
    /// Lua 5.4+: true for a `<close>` local, which its scope's exit uses even
    /// with no references.
    pub is_close: bool,
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

#[derive(Debug, Clone, Copy)]
pub struct Reference {
    pub span: Span,
    pub name: NameId,
    pub scope: ScopeId,
    /// The symbol this name resolves to, or `None` for a free name.
    pub resolved: Option<SymbolId>,
    /// Lua 5.2+: the local `_ENV` a free name is a field of, in which case
    /// it's no global.
    pub env: Option<SymbolId>,
    pub kind: ReferenceKind,
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

impl<'ast> ScopeTree<'ast> {
    /// One walk over `block`, which recurses once per level of nesting, into
    /// tables sized from `counts`, the parse's or the default.
    #[must_use]
    pub fn build(block: &'ast Block<'ast>, counts: TreeCounts, version: LuaVersion) -> Self {
        Self::build_with(block, counts, version, &mut ())
    }

    /// [`Self::build`], showing `sink` each node the walk reaches.
    #[must_use]
    pub fn build_with<Sink>(
        block: &'ast Block<'ast>,
        counts: TreeCounts,
        version: LuaVersion,
        sink: &mut Sink,
    ) -> Self
    where
        Sink: NodeSink<'ast>,
    {
        build_with_initializers(block, counts, version, sink).0
    }

    #[must_use]
    pub const fn version(&self) -> LuaVersion {
        self.version
    }

    /// The chunk's own scope first, then each in the order the walk enters
    /// it, so a parent comes before its children.
    #[must_use]
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    #[must_use]
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    #[must_use]
    pub fn references(&self) -> &[Reference] {
        &self.references
    }

    /// The chunk's distinct names, in [`NameId`] order.
    pub fn names(&self) -> impl ExactSizeIterator<Item = (NameId, &'ast str)> {
        self.names.iter().enumerate().map(|(i, entry)| (NameId::from_index(i), entry.text))
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this tree, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.index()]
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this tree, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.index()]
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this tree, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn reference(&self, id: ReferenceId) -> &Reference {
        &self.references[id.index()]
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this tree, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn name(&self, id: NameId) -> &'ast str {
        self.names[id.index()].text
    }

    /// `None` for a name the chunk never uses.
    #[must_use]
    pub fn name_id(&self, name: &str) -> Option<NameId> {
        self.name_ids.get(name).copied()
    }

    /// The references that resolve to `symbol`, in reference order.
    #[must_use]
    pub fn resolved_references(&self, symbol: SymbolId) -> &[ReferenceId] {
        let start = self.resolved_offsets.get(symbol.index()).copied().unwrap_or_default();
        let end = self.resolved_offsets.get(symbol.index().saturating_add(1)).copied();
        let run = offset_to_index(start)..offset_to_index(end.unwrap_or(start));
        self.resolved.get(run).unwrap_or_default()
    }

    /// The reference `name` makes: `None` for a name that makes none, such
    /// as a field, a declaration, or a name from another tree.
    #[must_use]
    pub fn reference_of(&self, name: &Name<'_>) -> Option<ReferenceId> {
        let start = name.span.start;
        let bucket = offset_to_index(start >> BUCKET_SHIFT);
        let candidates =
            match (self.buckets.get(bucket), self.buckets.get(bucket.saturating_add(1))) {
                (Some(&first), Some(&end)) => {
                    self.positions.get(offset_to_index(first)..offset_to_index(end))?
                }
                _ if self.buckets.is_empty() => &self.positions,
                _ => return None,
            };
        let first = candidates.partition_point(|position| position.start < start);
        // A built tree can give several names one span.
        candidates
            .get(first..)?
            .iter()
            .take_while(|position| position.start == start)
            .map(|position| position.reference)
            .find(|&id| {
                let reference = self.reference(id);
                reference.span == name.span && self.name(reference.name) == name.text
            })
    }

    /// The reference whose name holds `offset`, either end included, so an
    /// editor's cursor just past a name is on it. Names don't overlap, so
    /// it's the last one to start by then.
    #[must_use]
    pub fn reference_at(&self, offset: u32) -> Option<ReferenceId> {
        let after = self.positions.partition_point(|position| position.start <= offset);
        let last = self.positions.get(after.checked_sub(1)?)?.reference;
        (offset <= self.reference(last).span.end).then_some(last)
    }

    /// The references whose names lie within `span`, by where they start.
    pub fn references_within(&self, span: Span) -> impl Iterator<Item = ReferenceId> + '_ {
        let first = self.positions.partition_point(|position| position.start < span.start);
        self.positions
            .get(first..)
            .unwrap_or_default()
            .iter()
            .take_while(move |position| position.start < span.end)
            .map(|position| position.reference)
            .filter(move |&id| self.reference(id).span.end <= span.end)
    }

    /// Code that matches stdlib paths (`table.insert`, `require(...)`,
    /// `error(...)`) must bail when the base name is shadowed
    /// (`local table = {}`), or it reports stdlib diagnostics on user values.
    #[must_use]
    pub fn resolves_to_local(&self, name: &Name<'_>) -> bool {
        self.reference_of(name).is_some_and(|id| self.reference(id).resolved.is_some())
    }

    /// True when `name` reads the host's global of that name, so a stdlib
    /// fact about it holds there.
    ///
    /// False for a local, a field of a local `_ENV`, a chunk that assigns its
    /// own `_ENV`, a global this chunk writes anywhere, and a name that makes
    /// no reference.
    #[must_use]
    pub fn reaches_host_global(&self, name: &Name<'_>) -> bool {
        self.reference_of(name).is_some_and(|id| self.is_host_global(self.reference(id)))
    }

    /// See [`Self::reaches_host_global`].
    #[must_use]
    pub fn is_host_global(&self, reference: &Reference) -> bool {
        self.is_global(reference) && !self.is_assigned_global(reference.name)
    }

    /// Whether `reference` names a global. A free name that indexes a `_ENV`
    /// the chunk controls doesn't.
    #[must_use]
    pub const fn is_global(&self, reference: &Reference) -> bool {
        reference.resolved.is_none() && reference.env.is_none() && !self.is_env_assigned
    }

    /// Every reference to a global, in reference order.
    pub fn global_references(&self) -> impl Iterator<Item = &Reference> {
        self.references.iter().filter(|reference| self.is_global(reference))
    }

    /// Whether this chunk writes a global of this name as a free name, so
    /// the host's value of it can't be assumed anywhere in the chunk.
    #[must_use]
    pub fn is_assigned_global(&self, name: NameId) -> bool {
        self.names.get(name.index()).is_some_and(|entry| entry.is_assigned_global)
    }

    /// Lua 5.5: whether a `global` statement declares this name, which makes
    /// it defined wherever it's read.
    #[must_use]
    pub fn is_declared_global(&self, name: NameId) -> bool {
        self.names.get(name.index()).is_some_and(|entry| entry.is_declared_global)
    }

    /// Lua 5.2+: the chunk assigns its own `_ENV`, which redirects every
    /// free name. A local `_ENV` is [`Reference::env`] instead.
    #[must_use]
    pub const fn is_env_assigned(&self) -> bool {
        self.is_env_assigned
    }

    /// Whether `symbol` is a Lua 5.2+ `_ENV`, which every free name in its
    /// scope indexes.
    #[must_use]
    pub fn is_env_binding(&self, symbol: SymbolId) -> bool {
        self.version.has_env_upvalue() && self.name(self.symbol(symbol).name) == "_ENV"
    }

    /// Whether code in the scope `from` runs in a function nested inside the
    /// scope `to`, which encloses it, rather than in the function that holds
    /// `to`.
    #[must_use]
    pub fn crosses_function_boundary(&self, from: ScopeId, to: ScopeId) -> bool {
        self.scope(from).function != self.scope(to).function
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;

    use super::*;
    use crate::fixtures::parse;

    #[test]
    fn reference_at_finds_the_name_an_offset_touches() {
        let allocator = Allocator::new();
        let source = "local ab = 1 print(ab)";
        let tree = ScopeTree::build(
            parse(&allocator, source, LuaVersion::Lua54),
            TreeCounts::default(),
            LuaVersion::Lua54,
        );
        let name_at = |offset| tree.reference_at(offset).map(|id| tree.reference(id).span);
        // `print` is 13..18 and `ab` 19..21.
        assert_eq!(name_at(13), Some(Span::new(13, 18)));
        assert_eq!(name_at(18), Some(Span::new(13, 18)));
        assert_eq!(name_at(19), Some(Span::new(19, 21)));
        assert_eq!(name_at(21), Some(Span::new(19, 21)));
        assert_eq!(name_at(22), None);
        // The declaration makes no reference.
        assert_eq!(name_at(6), None);
        assert_eq!(name_at(0), None);
    }

    #[test]
    fn ids_are_niche_optimized() {
        assert_eq!(size_of::<Option<ScopeId>>(), 4);
        assert_eq!(size_of::<Option<SymbolId>>(), 4);
        assert_eq!(size_of::<Option<ReferenceId>>(), 4);
        assert_eq!(size_of::<Option<NameId>>(), 4);
    }

    #[test]
    fn id_roundtrips_index() {
        let id = SymbolId::from_index(7);
        assert_eq!(id.index(), 7);
        assert_eq!(format!("{id:?}"), "SymbolId(7)");
    }

    #[test]
    fn table_rows_stay_small() {
        assert_eq!(size_of::<Reference>(), 28);
        assert_eq!(size_of::<Symbol>(), 32);
        assert_eq!(size_of::<Scope>(), 12);
    }
}
