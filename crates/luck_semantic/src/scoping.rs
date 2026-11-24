use core::mem;

use luck_ast::{Name, NameIndex, TreeCounts};
use luck_token::{LuaVersion, Span, offset_to_index};

use crate::id::{LabelId, NameId, NodeId, ReferenceId, ScopeId, SymbolId};
use crate::names::Names;

/// A chunk's scopes, symbols, references, and labels, in flat tables indexed
/// by their ids, and the distinct names they use.
///
/// It owns all of it, so it outlives the tree it was built from: a pass that
/// rewrites the tree can keep it.
///
/// Every name in the tree that declares a symbol, makes a reference, or
/// names a label finds it by its [`NameIndex`], which
/// the parser gives each name of its own, so a tree whose names share an
/// index, as a rewrite that copies a name may leave, finds the role of only
/// the last of them.
///
/// Symbols are in the order the walk declares them, and references in the
/// order the chunk evaluates them, which is source order except that an
/// assignment writes a name only after its values are read.
#[derive(Debug)]
pub struct Scoping {
    version: LuaVersion,
    scopes: Vec<Scope>,
    symbols: Vec<Symbol>,
    references: Vec<Reference>,
    labels: Vec<Label>,
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
    is_env_assigned: bool,
}

/// A slot of the references by position that holds no reference yet. No
/// chunk has as many references as its last index, since it has more bytes.
fn unfilled_position() -> ReferenceId {
    ReferenceId::from_index(usize::MAX)
}

/// What a name in the tree is to the tables.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum NameRole {
    /// A field, a type, an attribute, a global's declaration, or a name the
    /// walk never reached.
    #[default]
    None,
    Symbol(SymbolId),
    Reference(ReferenceId),
    /// A label's own name, or a `goto`'s that reaches it.
    Label(LabelId),
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
    /// nested function's, reads `...`. Lua 5.1 builds a vararg function's
    /// `arg` table only when it doesn't.
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "five independent facts about a binding, in any combination"
)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: NameId,
    pub scope: ScopeId,
    pub kind: SymbolKind,
    /// The node that declares it: a local's [`AttributedName`], a parameter
    /// or generic `for` variable's [`Parameter`], a numeric `for`, a
    /// `local function`, the [`VarArgParam`] of a Lua 5.5 named vararg or a
    /// Lua 5.1 `arg`, or the [`FunctionBody`] of a method's `self`.
    ///
    /// [`AttributedName`]: luck_ast::AttributedName
    /// [`Parameter`]: luck_ast::Parameter
    /// [`VarArgParam`]: luck_ast::VarArgParam
    /// [`FunctionBody`]: luck_ast::FunctionBody
    pub declaration: NodeId,
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
    /// Writing to it is a compile error: a `<const>` or `<close>` local, a
    /// Luau `const` or `export function`, a Lua 5.5 `for` loop's control
    /// variable, or a Lua 5.5 named vararg.
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
    /// Lua 5.1: the `arg` table of a vararg function's extra arguments. No
    /// token spells it, so its definition span is empty, at the `...`.
    ImplicitArg,
}

impl SymbolKind {
    /// Whether no token spells the symbol, so it has no name to rename or
    /// remove.
    #[must_use]
    pub const fn is_implicit(self) -> bool {
        match self {
            Self::ImplicitSelf | Self::ImplicitArg => true,
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

/// Lua 5.2+: a `::name::` statement, which the `goto`s of its function that
/// can see it jump to.
#[derive(Debug, Clone, Copy)]
pub struct Label {
    pub name: NameId,
    /// The whole statement's.
    pub span: Span,
    /// The scope of the block it's in.
    pub scope: ScopeId,
    pub node: NodeId,
    /// A `goto` jumps to it.
    pub is_used: bool,
}

impl Scoping {
    /// Tables sized from `counts`, the parse's or the default.
    pub(crate) fn with_capacity(version: LuaVersion, counts: TreeCounts) -> Self {
        let declarations = offset_to_index(counts.declarations);
        let name_uses = offset_to_index(counts.name_uses);
        let mut resolved_offsets = Vec::with_capacity(declarations.saturating_add(1));
        resolved_offsets.push(0);
        Self {
            version,
            scopes: Vec::with_capacity(offset_to_index(counts.blocks)),
            symbols: Vec::with_capacity(declarations),
            references: Vec::with_capacity(name_uses),
            labels: Vec::new(),
            resolved: Vec::new(),
            resolved_offsets,
            positions: Vec::with_capacity(name_uses),
            unfilled_positions: 0,
            roles: vec![NameRole::None; offset_to_index(counts.names)],
            // No chunk has more distinct names than declarations and uses
            // together.
            names: Names::with_capacity(declarations.saturating_add(name_uses)),
            is_env_assigned: false,
        }
    }

    pub(crate) fn add_scope(&mut self, scope: Scope) -> ScopeId {
        let id = ScopeId::from_index(self.scopes.len());
        self.scopes.push(scope);
        id
    }

    pub(crate) fn mark_vararg_read(&mut self, function: ScopeId) {
        if let Some(scope) = self.scopes.get_mut(function.index()) {
            scope.has_vararg_read = true;
        }
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
            is_close: false,
            is_read_only: false,
        });
        self.resolved_offsets.push(0);
        id
    }

    pub(crate) fn symbol_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.symbols.get_mut(id.index())
    }

    /// A slot in the references by position for one the walk adds once it's
    /// past the name that makes it, as it does a name an assignment writes.
    pub(crate) fn reserve_position(&mut self) -> usize {
        let slot = self.positions.len();
        self.positions.push(unfilled_position());
        self.unfilled_positions = self.unfilled_positions.saturating_add(1);
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
        if let Some(count) = reference
            .resolved
            .and_then(|symbol| self.resolved_offsets.get_mut(symbol.index().saturating_add(1)))
        {
            *count = count.saturating_add(1);
        }
        self.references.push(reference);
        if let Some(slot) = position.and_then(|slot| self.positions.get_mut(slot)) {
            *slot = id;
            self.unfilled_positions = self.unfilled_positions.saturating_sub(1);
        } else {
            self.positions.push(id);
        }
        id
    }

    pub(crate) fn add_label(&mut self, label: Label) -> LabelId {
        let id = LabelId::from_index(self.labels.len());
        self.labels.push(label);
        id
    }

    pub(crate) fn label_mut(&mut self, id: LabelId) -> Option<&mut Label> {
        self.labels.get_mut(id.index())
    }

    pub(crate) fn intern(&mut self, name: &str) -> NameId {
        self.names.intern(name)
    }

    pub(crate) fn mark_assigned_global(&mut self, name: NameId) {
        self.names.mark_assigned_global(name);
    }

    pub(crate) fn mark_declared_global(&mut self, name: NameId) {
        self.names.mark_declared_global(name);
    }

    /// Records what the name of index `name_index` is, growing the table for
    /// a tree the parse didn't count.
    pub(crate) fn set_role(&mut self, name_index: Option<NameIndex>, role: NameRole) {
        let Some(index) = name_index.map(NameIndex::get).map(offset_to_index) else {
            return;
        };
        if self.roles.len() <= index {
            self.roles.resize(index.saturating_add(1), NameRole::None);
        }
        if let Some(slot) = self.roles.get_mut(index) {
            *slot = role;
        }
    }

    /// Lays out each symbol's references once the walk has added them all.
    pub(crate) fn finish(&mut self) {
        let Self { references, resolved, resolved_offsets, positions, .. } = self;
        // Each symbol's count, one slot along, becomes the start of its run,
        // which then fills in reference order up to where the next starts.
        let mut total = 0u32;
        for offset in resolved_offsets.iter_mut().skip(1) {
            let count = mem::replace(offset, total);
            total = total.saturating_add(count);
        }
        *resolved = vec![ReferenceId::from_index(0); offset_to_index(total)];
        for (i, reference) in references.iter().enumerate() {
            if let Some(cursor) = reference
                .resolved
                .and_then(|symbol| resolved_offsets.get_mut(symbol.index().saturating_add(1)))
                && let Some(slot) = resolved.get_mut(offset_to_index(*cursor))
            {
                *slot = ReferenceId::from_index(i);
                *cursor = cursor.saturating_add(1);
            }
        }
        // A slot a name reserved but that made no reference, which a name the
        // parser found missing does, holds none.
        if self.unfilled_positions > 0 {
            let unfilled = unfilled_position();
            positions.retain(|&id| id != unfilled);
        }
        self.is_env_assigned = self.version.has_env_upvalue()
            && self.name_id("_ENV").is_some_and(|env| self.is_assigned_global(env));
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

    /// In source order.
    #[must_use]
    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    /// The chunk's distinct names, in [`NameId`] order.
    pub fn names(&self) -> impl ExactSizeIterator<Item = (NameId, &str)> {
        (0..self.names.len()).map(|i| {
            let id = NameId::from_index(i);
            (id, self.names.text(id))
        })
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this table, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.index()]
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this table, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.index()]
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this table, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn reference(&self, id: ReferenceId) -> &Reference {
        &self.references[id.index()]
    }

    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "ids are minted only by this table, and a fallback would turn a bug into wrong diagnostics"
    )]
    pub fn label(&self, id: LabelId) -> &Label {
        &self.labels[id.index()]
    }

    #[must_use]
    pub fn name(&self, id: NameId) -> &str {
        self.names.text(id)
    }

    /// `None` for a name the chunk never uses.
    #[must_use]
    pub fn name_id(&self, name: &str) -> Option<NameId> {
        self.names.get(name)
    }

    /// The references that resolve to `symbol`, in reference order.
    #[must_use]
    pub fn resolved_references(&self, symbol: SymbolId) -> &[ReferenceId] {
        let start = self.resolved_offsets.get(symbol.index()).copied().unwrap_or_default();
        let end = self.resolved_offsets.get(symbol.index().saturating_add(1)).copied();
        let run = offset_to_index(start)..offset_to_index(end.unwrap_or(start));
        self.resolved.get(run).unwrap_or_default()
    }

    fn role(&self, name: &Name<'_>) -> NameRole {
        name.index
            .and_then(|index| self.roles.get(offset_to_index(index.get())))
            .copied()
            .unwrap_or_default()
    }

    /// The symbol `name` declares: `None` for a name that declares none.
    #[must_use]
    pub fn symbol_of(&self, name: &Name<'_>) -> Option<SymbolId> {
        let NameRole::Symbol(id) = self.role(name) else {
            return None;
        };
        Some(id)
    }

    /// The reference `name` makes: `None` for a name that makes none, such
    /// as a field or a declaration.
    #[must_use]
    pub fn reference_of(&self, name: &Name<'_>) -> Option<ReferenceId> {
        let NameRole::Reference(id) = self.role(name) else {
            return None;
        };
        Some(id)
    }

    /// The label `name` names: its own, or the one a `goto` with that name
    /// jumps to.
    #[must_use]
    pub fn label_of(&self, name: &Name<'_>) -> Option<LabelId> {
        let NameRole::Label(id) = self.role(name) else {
            return None;
        };
        Some(id)
    }

    /// The reference whose name holds `offset`, either end included, so an
    /// editor's cursor just past a name is on it. Names don't overlap, so
    /// it's the last one to start by then.
    #[must_use]
    pub fn reference_at(&self, offset: u32) -> Option<ReferenceId> {
        let after = self.positions.partition_point(|&id| self.reference(id).span.start <= offset);
        let last = *self.positions.get(after.checked_sub(1)?)?;
        (offset <= self.reference(last).span.end).then_some(last)
    }

    /// The references whose names lie within `span`, by where they start.
    pub fn references_within(&self, span: Span) -> impl Iterator<Item = ReferenceId> + '_ {
        let first =
            self.positions.partition_point(|&id| self.reference(id).span.start < span.start);
        self.positions
            .get(first..)
            .unwrap_or_default()
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
    #[must_use]
    pub fn nearest_local(&self, name: &str, offset: u32) -> Option<&Symbol> {
        let name_id = self.name_id(name)?;
        self.symbols
            .iter()
            .filter(|symbol| symbol.name == name_id && symbol.definition_span.end <= offset)
            .max_by_key(|symbol| symbol.definition_span.start)
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
        self.names.is_assigned_global(name)
    }

    /// Lua 5.5: whether a `global` statement declares this name, which makes
    /// it defined wherever it's read.
    #[must_use]
    pub fn is_declared_global(&self, name: NameId) -> bool {
        self.names.is_declared_global(name)
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
    use super::*;

    #[test]
    fn table_rows_stay_small() {
        assert_eq!(size_of::<Reference>(), 32);
        assert_eq!(size_of::<Symbol>(), 36);
        assert_eq!(size_of::<Scope>(), 16);
        assert_eq!(size_of::<Label>(), 24);
        assert_eq!(size_of::<NameRole>(), 8);
    }
}
