use std::mem;

use luck_ast::{Name, NameIndex, TreeCounts};
use luck_token::Span;

use crate::id::{NameId, NodeId, ReferenceId, ScopeId, SymbolId};
use crate::names::Names;

/// A chunk's scopes, symbols, and references, in flat tables indexed by
/// their ids, and the distinct names they use.
///
/// It owns all of it, so it outlives the tree it was built from: a pass that
/// rewrites the tree can keep it.
///
/// Every name in the tree that declares a symbol or makes a reference finds
/// it by its [`NameIndex`], which the parser gives each name of its own, so
/// a tree whose names share an index, as a rewrite that copies a name may
/// leave, finds the role of only the last of them.
///
/// Symbols are in the order the walk declares them, and references in the
/// order the chunk evaluates them, which is source order except that an
/// assignment writes a name only after its values are read.
#[derive(Debug)]
pub struct Scoping {
    scopes: Vec<Scope>,
    symbols: Vec<Symbol>,
    references: Vec<Reference>,
    /// Every symbol's references, a run per symbol in symbol order.
    resolved: Vec<ReferenceId>,
    /// Where each symbol's run of `resolved` starts, and then where the last
    /// one ends. Until [`Self::finish`], each symbol's count, one slot along.
    resolved_offsets: Vec<u32>,
    /// Every reference, by where its name starts.
    positions: Vec<ReferenceId>,
    /// How many slots of `positions` the walk reserved and hasn't filled.
    unfilled_positions: u32,
    /// What each name of the tree is, by its index.
    roles: Vec<NameRole>,
    names: Names,
}

/// A slot of the references by position that holds no reference yet. No
/// chunk has as many references as its last index, since it has more bytes.
fn unfilled_position() -> ReferenceId {
    ReferenceId::from_index(usize::MAX)
}

/// What a name in the tree is to the tables.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum NameRole {
    /// A field, a type, a function attribute, or a name the walk never
    /// reached.
    #[default]
    None,
    Symbol(SymbolId),
    Reference(ReferenceId),
}

#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    /// The function scope this one runs in: itself for a function or the
    /// chunk.
    pub function: ScopeId,
    /// What opens it: the chunk, a function body, the block of a `do`, a
    /// `while`, or an `if` clause, or a `repeat` or `for` loop.
    pub node: NodeId,
    /// For the chunk's scope or a function's, whether its own code, not a
    /// nested function's, reads `...`.
    pub has_vararg_read: bool,
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
pub struct Symbol {
    pub id: SymbolId,
    pub name: NameId,
    pub scope: ScopeId,
    pub kind: SymbolKind,
    /// The node that declares it: a local's [`LocalName`], a parameter or
    /// generic `for` variable's [`Parameter`], a numeric `for`, a
    /// `local function`, or the [`FunctionBody`] of a method's `self`.
    ///
    /// [`LocalName`]: luck_ast::LocalName
    /// [`Parameter`]: luck_ast::Parameter
    /// [`FunctionBody`]: luck_ast::FunctionBody
    pub declaration: NodeId,
    pub definition_span: Span,
    pub shadows: Option<SymbolId>,
    pub is_upvalue: bool,
    pub is_modified: bool,
    /// True for `export local`, `export const`, and `export function`
    /// bindings. The module's export table reads them, so they're used even
    /// with no references in the module, and their name is the module's API.
    pub is_exported: bool,
    /// Writing to it is an error: a `const` local or function, or an
    /// `export const` or `export function`.
    pub is_read_only: bool,
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

impl SymbolKind {
    /// Whether no token spells the symbol, so it has no name to rename or
    /// remove.
    pub const fn is_implicit(self) -> bool {
        match self {
            Self::ImplicitSelf => true,
            Self::Local
            | Self::Parameter
            | Self::IteratorVariable
            | Self::NumericForVariable
            | Self::FunctionName => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Reference {
    pub span: Span,
    pub name: NameId,
    pub scope: ScopeId,
    /// The [`Name`] that makes it.
    pub node: NodeId,
    /// The symbol this name resolves to, or `None` for a global.
    pub resolved: Option<SymbolId>,
    pub kind: ReferenceKind,
    /// Made in a type, as a `typeof` makes one, which never runs.
    pub is_in_type: bool,
    /// Made in the body of the `local function` it resolves to.
    pub is_recursive: bool,
}

impl Reference {
    /// Whether it names a global.
    pub const fn is_global(&self) -> bool {
        self.resolved.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    Read,
    Write,
    ReadWrite,
}

impl ReferenceKind {
    pub const fn is_read(self) -> bool {
        match self {
            Self::Read | Self::ReadWrite => true,
            Self::Write => false,
        }
    }

    pub const fn is_write(self) -> bool {
        match self {
            Self::Write | Self::ReadWrite => true,
            Self::Read => false,
        }
    }
}

impl Scoping {
    /// Tables sized from `counts`, the parse's or the default.
    pub(crate) fn with_capacity(counts: TreeCounts) -> Self {
        let declarations = counts.declarations as usize;
        let name_uses = counts.name_uses as usize;
        let mut resolved_offsets = Vec::with_capacity(declarations + 1);
        resolved_offsets.push(0);
        Self {
            scopes: Vec::with_capacity(counts.blocks as usize),
            symbols: Vec::with_capacity(declarations),
            references: Vec::with_capacity(name_uses),
            resolved: Vec::new(),
            resolved_offsets,
            positions: Vec::with_capacity(name_uses),
            unfilled_positions: 0,
            roles: vec![NameRole::None; counts.names as usize],
            // No chunk has more distinct names than declarations and uses
            // together.
            names: Names::with_capacity(declarations + name_uses),
        }
    }

    /// Empties every table and keeps its memory, for a build of a tree whose
    /// `counts` it then reserves room for, as [`Self::with_capacity`] does.
    pub(crate) fn reset(&mut self, counts: TreeCounts) {
        let declarations = counts.declarations as usize;
        let name_uses = counts.name_uses as usize;
        let Self {
            scopes,
            symbols,
            references,
            resolved,
            resolved_offsets,
            positions,
            unfilled_positions,
            roles,
            names,
        } = self;
        scopes.clear();
        scopes.reserve(counts.blocks as usize);
        symbols.clear();
        symbols.reserve(declarations);
        references.clear();
        references.reserve(name_uses);
        resolved.clear();
        resolved_offsets.clear();
        resolved_offsets.reserve(declarations + 1);
        resolved_offsets.push(0);
        positions.clear();
        positions.reserve(name_uses);
        *unfilled_positions = 0;
        roles.clear();
        roles.resize(counts.names as usize, NameRole::None);
        names.reset(declarations + name_uses);
    }

    pub(crate) fn next_scope_id(&self) -> ScopeId {
        ScopeId::from_index(self.scopes.len())
    }

    pub(crate) fn add_scope(&mut self, scope: Scope) {
        self.scopes.push(scope);
    }

    pub(crate) fn mark_vararg_read(&mut self, function: ScopeId) {
        self.scopes[function.index()].has_vararg_read = true;
    }

    pub(crate) fn add_symbol(
        &mut self,
        name: NameId,
        scope: ScopeId,
        kind: SymbolKind,
        declaration: NodeId,
        definition_span: Span,
        shadows: Option<SymbolId>,
    ) -> SymbolId {
        let id = SymbolId::from_index(self.symbols.len());
        self.symbols.push(Symbol {
            id,
            name,
            scope,
            kind,
            declaration,
            definition_span,
            shadows,
            is_upvalue: false,
            is_modified: false,
            is_exported: false,
            is_read_only: false,
        });
        self.resolved_offsets.push(0);
        id
    }

    pub(crate) fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
        &mut self.symbols[id.index()]
    }

    /// A slot in the references by position for one the walk adds once it's
    /// past the name that makes it, as it does a name an assignment writes.
    pub(crate) fn reserve_position(&mut self) -> usize {
        let slot = self.positions.len();
        self.positions.push(unfilled_position());
        self.unfilled_positions += 1;
        slot
    }

    /// Adds `reference`, which its symbol counts among its own, at the end
    /// of the references by position, or in `position`, a slot
    /// [`Self::reserve_position`] gave.
    pub(crate) fn add_reference(
        &mut self,
        reference: Reference,
        position: Option<usize>,
    ) -> ReferenceId {
        let id = ReferenceId::from_index(self.references.len());
        if let Some(symbol) = reference.resolved {
            self.resolved_offsets[symbol.index() + 1] += 1;
        }
        self.references.push(reference);
        if let Some(slot) = position {
            self.positions[slot] = id;
            self.unfilled_positions -= 1;
        } else {
            self.positions.push(id);
        }
        id
    }

    pub(crate) fn intern(&mut self, name: &str) -> NameId {
        self.names.intern(name)
    }

    pub(crate) fn mark_assigned_global(&mut self, name: NameId) {
        self.names.mark_assigned_global(name);
    }

    /// Records what the name of index `name_index` is, growing the table for
    /// a tree the parse didn't count.
    pub(crate) fn set_role(&mut self, name_index: Option<NameIndex>, role: NameRole) {
        let Some(index) = name_index else {
            return;
        };
        let index = index.get() as usize;
        if self.roles.len() <= index {
            self.roles.resize(index + 1, NameRole::None);
        }
        self.roles[index] = role;
    }

    /// Keeps the references `keep` takes in each symbol's list, in order, for
    /// a pass that rewrites the tree and drops some of the names that made
    /// them.
    pub fn retain_resolved_references<Keep>(&mut self, mut keep: Keep)
    where
        Keep: FnMut(ReferenceId) -> bool,
    {
        let Self { resolved, resolved_offsets, .. } = self;
        let mut kept: u32 = 0;
        let mut start = 0;
        for offset in resolved_offsets.iter_mut().skip(1) {
            let end = *offset as usize;
            for i in start..end {
                let id = resolved[i];
                if keep(id) {
                    resolved[kept as usize] = id;
                    kept += 1;
                }
            }
            *offset = kept;
            start = end;
        }
        resolved.truncate(kept as usize);
    }

    /// Lays out each symbol's references once the walk has added them all.
    pub(crate) fn finish(&mut self) {
        let Self { references, resolved, resolved_offsets, positions, .. } = self;
        // Each symbol's count, one slot along, becomes the start of its run,
        // which then fills in reference order up to where the next starts.
        let mut total = 0;
        for offset in resolved_offsets.iter_mut().skip(1) {
            let count = mem::replace(offset, total);
            total += count;
        }
        resolved.resize(total as usize, ReferenceId::from_index(0));
        for (i, reference) in references.iter().enumerate() {
            if let Some(symbol) = reference.resolved {
                let cursor = &mut resolved_offsets[symbol.index() + 1];
                resolved[*cursor as usize] = ReferenceId::from_index(i);
                *cursor += 1;
            }
        }
        // A slot a name reserved but that made no reference, which a name the
        // parser found missing does, holds none.
        if self.unfilled_positions > 0 {
            let unfilled = unfilled_position();
            positions.retain(|&id| id != unfilled);
        }
    }

    /// The chunk's own scope first, then each in the order the walk enters
    /// it, so a parent comes before its children.
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    pub fn references(&self) -> &[Reference] {
        &self.references
    }

    /// The chunk's distinct names, in [`NameId`] order.
    pub fn names(&self) -> impl ExactSizeIterator<Item = (NameId, &str)> {
        (0..self.names.len()).map(|i| {
            let id = NameId::from_index(i);
            (id, self.names.text(id))
        })
    }

    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.index()]
    }

    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.index()]
    }

    pub fn reference(&self, id: ReferenceId) -> &Reference {
        &self.references[id.index()]
    }

    pub fn name(&self, id: NameId) -> &str {
        self.names.text(id)
    }

    /// `None` for a name the chunk never uses.
    pub fn name_id(&self, name: &str) -> Option<NameId> {
        self.names.get(name)
    }

    /// The references that resolve to `symbol`, in reference order.
    pub fn resolved_references(&self, symbol: SymbolId) -> &[ReferenceId] {
        let start = self.resolved_offsets[symbol.index()] as usize;
        let end = self.resolved_offsets[symbol.index() + 1] as usize;
        &self.resolved[start..end]
    }

    fn role(&self, name: &Name<'_>) -> NameRole {
        name.index
            .and_then(|index| self.roles.get(index.get() as usize))
            .copied()
            .unwrap_or_default()
    }

    /// The symbol `name` declares: `None` for a name that declares none.
    pub fn symbol_of(&self, name: &Name<'_>) -> Option<SymbolId> {
        let NameRole::Symbol(id) = self.role(name) else {
            return None;
        };
        Some(id)
    }

    /// The reference `name` makes: `None` for a name that makes none, such
    /// as a field or a declaration.
    pub fn reference_of(&self, name: &Name<'_>) -> Option<ReferenceId> {
        let NameRole::Reference(id) = self.role(name) else {
            return None;
        };
        Some(id)
    }

    /// The symbol the reference `name` makes resolves to: `None` for a name
    /// that makes none, or that reads a global.
    pub fn resolved_symbol_of(&self, name: &Name<'_>) -> Option<SymbolId> {
        self.reference(self.reference_of(name)?).resolved
    }

    /// The references whose names lie within `span`, by where they start.
    pub fn references_within(&self, span: Span) -> impl Iterator<Item = ReferenceId> + '_ {
        let first =
            self.positions.partition_point(|&id| self.reference(id).span.start < span.start);
        self.positions[first..]
            .iter()
            .copied()
            .take_while(move |&id| self.reference(id).span.start < span.end)
            .filter(move |&id| self.reference(id).span.end <= span.end)
    }

    /// The last local named `name` declared before `offset`, for text that
    /// makes no reference, such as an editor's half-typed `f:` or a fix's
    /// replacement.
    ///
    /// Scope isn't checked, so a local from a block that already closed can
    /// still match.
    pub fn nearest_local(&self, name: &str, offset: u32) -> Option<&Symbol> {
        let name_id = self.name_id(name)?;
        self.symbols
            .iter()
            .filter(|symbol| symbol.name == name_id && symbol.definition_span.end <= offset)
            .max_by_key(|symbol| symbol.definition_span.start)
    }

    /// Whether `name` makes a reference to a local, as `table` does after
    /// `local table = {}`. Code that matches stdlib paths (`table.insert`,
    /// `require(...)`, `error(...)`) must bail when the base name does, or it
    /// reports stdlib diagnostics on user values.
    pub fn resolves_to_local(&self, name: &Name<'_>) -> bool {
        self.resolved_symbol_of(name).is_some()
    }

    /// True when `name` reads the host's global of that name, so a stdlib
    /// fact about it holds there.
    ///
    /// False for a local, a global this chunk writes anywhere, and a name that
    /// makes no reference.
    pub fn reaches_host_global(&self, name: &Name<'_>) -> bool {
        self.reference_of(name).is_some_and(|id| self.is_host_global(self.reference(id)))
    }

    /// See [`Self::reaches_host_global`].
    pub fn is_host_global(&self, reference: &Reference) -> bool {
        reference.is_global() && !self.is_assigned_global(reference.name)
    }

    /// Every reference to a global, in reference order.
    pub fn global_references(&self) -> impl Iterator<Item = &Reference> {
        self.references.iter().filter(|reference| reference.is_global())
    }

    /// Whether this chunk writes a global of this name as a free name, so
    /// the host's value of it can't be assumed anywhere in the chunk.
    pub fn is_assigned_global(&self, name: NameId) -> bool {
        self.names.is_assigned_global(name)
    }

    /// Whether code in the scope `from` runs in a function nested inside the
    /// scope `to`, which encloses it, rather than in the function that holds
    /// `to`.
    pub fn crosses_function_boundary(&self, from: ScopeId, to: ScopeId) -> bool {
        self.scope(from).function != self.scope(to).function
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_rows_stay_small() {
        assert_eq!(size_of::<Reference>(), 28);
        assert_eq!(size_of::<Symbol>(), 36);
        assert_eq!(size_of::<Scope>(), 16);
        assert_eq!(size_of::<NameRole>(), 8);
    }
}
