use core::mem;
use std::collections::hash_map::Entry;

use luck_ast::{
    Assignment, Block, Chunk, CompoundAssignment, DoBlock, Expression, FunctionAttribute,
    FunctionBody, FunctionDecl, GenericFor, GlobalDeclaration, GlobalFunction, GlobalStar,
    IfStatement, LocalAssignment, LocalFunction, Name, NumericFor, RepeatLoop, Statement,
    SuffixedExpression, TableConstructor, TreeCounts, TypeDeclaration, TypeDeclarationValue, Var,
    Visit, WhileLoop,
};
use luck_token::{Span, index_to_offset, offset_to_index};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

use super::{
    BUCKET_SHIFT, MAX_BUCKETS_PER_REFERENCE, NameEntry, NameId, Position, Reference, ReferenceId,
    ReferenceKind, Scope, ScopeId, ScopeKind, ScopeTree, Symbol, SymbolId, SymbolKind,
};
use crate::sink::NodeSink;

type InitializedLocals<'ast> = Vec<(SymbolId, &'ast Expression<'ast>)>;

/// [`ScopeTree::build`], and each local that a `local` statement with one
/// initializer per name declares, paired with its initializer. `sink` sees
/// each node the walk reaches.
///
/// A local comes before any initializer that can read it.
pub(crate) fn build_with_initializers<'ast, Sink>(
    chunk: &'ast Chunk<'ast>,
    counts: TreeCounts,
    sink: &mut Sink,
) -> (ScopeTree<'ast>, InitializedLocals<'ast>)
where
    Sink: NodeSink<'ast>,
{
    build(chunk, counts, AllNames, sink)
}

/// [`ScopeTree::build_for_names`].
pub(crate) fn build_for_names<'ast, Sink>(
    chunk: &'ast Chunk<'ast>,
    names: &[&str],
    sink: &mut Sink,
) -> ScopeTree<'ast>
where
    Sink: NodeSink<'ast>,
{
    build(chunk, TreeCounts::default(), NameSet::new(names), sink).0
}

/// Which names a build records.
trait NameFilter {
    fn keeps(&self, text: &str) -> bool;
}

struct AllNames;

impl NameFilter for AllNames {
    fn keeps(&self, _text: &str) -> bool {
        true
    }
}

/// Some names and `_ENV`, which decides what every free name reaches, with
/// each one's [`name_bit`] set, so one test turns most other names away.
struct NameSet<'names> {
    names: &'names [&'names str],
    bits: u64,
}

impl<'names> NameSet<'names> {
    fn new(names: &'names [&'names str]) -> Self {
        let bits = names.iter().fold(name_bit("_ENV"), |bits, name| bits | name_bit(name));
        Self { names, bits }
    }
}

impl NameFilter for NameSet<'_> {
    fn keeps(&self, text: &str) -> bool {
        self.bits & name_bit(text) != 0 && (text == "_ENV" || self.names.contains(&text))
    }
}

/// A bit picked by `text`'s first and last bytes and its length.
fn name_bit(text: &str) -> u64 {
    let bytes = text.as_bytes();
    let first = u32::from(bytes.first().copied().unwrap_or_default());
    let last = u32::from(bytes.last().copied().unwrap_or_default());
    1u64.wrapping_shl(
        first.wrapping_mul(3).wrapping_add(last).wrapping_add(index_to_offset(bytes.len())),
    )
}

fn build<'ast, Sink, Filter>(
    chunk: &'ast Chunk<'ast>,
    counts: TreeCounts,
    filter: Filter,
    sink: &mut Sink,
) -> (ScopeTree<'ast>, InitializedLocals<'ast>)
where
    Sink: NodeSink<'ast>,
    Filter: NameFilter,
{
    let module = ScopeId::from_index(0);
    let declarations = offset_to_index(counts.declarations);
    let name_uses = offset_to_index(counts.name_uses);
    // No chunk has more distinct names than declarations and uses together,
    // and a name table that loose probes less than an exact one.
    let names = declarations.saturating_add(name_uses);
    let mut scopes = Vec::with_capacity(offset_to_index(counts.blocks));
    scopes.push(Scope { parent: None, kind: ScopeKind::Module, function: module });
    let mut resolved_offsets = Vec::with_capacity(declarations.saturating_add(1));
    resolved_offsets.push(0);
    let mut builder = Builder {
        tree: ScopeTree {
            version: chunk.version,
            scopes,
            symbols: Vec::with_capacity(declarations),
            references: Vec::with_capacity(name_uses),
            resolved: Vec::new(),
            resolved_offsets,
            positions: Vec::with_capacity(name_uses),
            buckets: Vec::new(),
            by_index: vec![None; offset_to_index(counts.names)],
            is_indexed: true,
            names: Vec::with_capacity(names),
            name_ids: FxHashMap::with_capacity_and_hasher(names, FxBuildHasher),
            is_env_assigned: false,
        },
        bindings: Vec::with_capacity(names),
        current_scope: module,
        current_function: module,
        outer_scopes: Vec::new(),
        shadowed: Vec::new(),
        env_name: None,
        is_global_star: false,
        is_in_type_function: false,
        type_names: FxHashSet::default(),
        initialized_locals: Vec::new(),
        filter,
        sink,
    };
    builder.visit_block(&chunk.block);
    builder.finish()
}

/// A name's innermost live declaration.
#[derive(Debug, Clone, Copy)]
enum Binding {
    Free,
    Local(SymbolId),
    /// Lua 5.5: a `global` declaration, which voids any enclosing local of
    /// the name for the rest of its block.
    Global,
}

impl Binding {
    const fn local(self) -> Option<SymbolId> {
        match self {
            Self::Local(id) => Some(id),
            Self::Free | Self::Global => None,
        }
    }
}

/// What a scope's exit restores.
#[derive(Debug, Clone, Copy)]
struct OuterScope {
    scope: ScopeId,
    function: ScopeId,
    shadowed_len: usize,
    is_global_star: bool,
}

struct Builder<'ast, 'sink, Sink, Filter> {
    tree: ScopeTree<'ast>,
    /// Each name's innermost live declaration, by name. Resolving a name is
    /// one hash of it, and leaving a scope hashes nothing.
    bindings: Vec<Binding>,
    // The scope stack is never empty, so its top lives here and reading it
    // doesn't branch.
    current_scope: ScopeId,
    current_function: ScopeId,
    outer_scopes: Vec<OuterScope>,
    /// Undo log: the binding each declaration since the outermost open
    /// scope's entry replaced.
    shadowed: Vec<(NameId, Binding)>,
    /// Lua 5.2+: `_ENV`, once a local declares it.
    env_name: Option<NameId>,
    /// Lua 5.5: a `global *` is live.
    is_global_star: bool,
    is_in_type_function: bool,
    /// Luau: every type declared by a block entered so far.
    type_names: FxHashSet<&'ast str>,
    initialized_locals: InitializedLocals<'ast>,
    filter: Filter,
    sink: &'sink mut Sink,
}

impl<'ast, Sink, Filter> Builder<'ast, '_, Sink, Filter>
where
    Sink: NodeSink<'ast>,
    Filter: NameFilter,
{
    fn finish(mut self) -> (ScopeTree<'ast>, InitializedLocals<'ast>) {
        let ScopeTree {
            references,
            resolved,
            resolved_offsets,
            positions,
            buckets,
            is_indexed,
            ..
        } = &mut self.tree;
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
        // A parsed tree's are already sorted. A built tree's spans needn't
        // follow its source order.
        if !positions.is_sorted_by_key(|position| position.start) {
            positions.sort_by_key(|position| position.start);
        }
        let bucket_count = positions
            .last()
            .map_or(0, |last| offset_to_index(last.start >> BUCKET_SHIFT).saturating_add(1));
        if bucket_count <= positions.len().saturating_mul(MAX_BUCKETS_PER_REFERENCE) && !*is_indexed
        {
            buckets.reserve_exact(bucket_count.saturating_add(1));
            for (i, position) in (0u32..).zip(positions.iter()) {
                let bucket = offset_to_index(position.start >> BUCKET_SHIFT);
                if buckets.len() <= bucket {
                    buckets.resize(bucket.saturating_add(1), i);
                }
            }
            buckets.push(index_to_offset(positions.len()));
        }
        self.tree.is_env_assigned = self.tree.version.has_env_upvalue()
            && self.tree.name_id("_ENV").is_some_and(|env| self.tree.is_assigned_global(env));
        (self.tree, self.initialized_locals)
    }

    /// `None` for a name the build doesn't record.
    fn intern(&mut self, text: &'ast str) -> Option<NameId> {
        if !self.filter.keeps(text) {
            return None;
        }
        Some(match self.tree.name_ids.entry(text) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let id = NameId::from_index(self.tree.names.len());
                entry.insert(id);
                self.tree.names.push(NameEntry {
                    text,
                    is_assigned_global: false,
                    is_declared_global: false,
                });
                self.bindings.push(Binding::Free);
                id
            }
        })
    }

    fn binding(&self, name: NameId) -> Binding {
        self.bindings.get(name.index()).copied().unwrap_or(Binding::Free)
    }

    fn bind(&mut self, name: NameId, binding: Binding) {
        if let Some(slot) = self.bindings.get_mut(name.index()) {
            self.shadowed.push((name, mem::replace(slot, binding)));
        }
    }

    fn push_scope(&mut self, kind: ScopeKind) {
        let id = ScopeId::from_index(self.tree.scopes.len());
        let function = if kind == ScopeKind::Function { id } else { self.current_function };
        self.tree.scopes.push(Scope { parent: Some(self.current_scope), kind, function });
        self.outer_scopes.push(OuterScope {
            scope: mem::replace(&mut self.current_scope, id),
            function: mem::replace(&mut self.current_function, function),
            shadowed_len: self.shadowed.len(),
            is_global_star: self.is_global_star,
        });
    }

    fn pop_scope(&mut self) {
        let Some(outer) = self.outer_scopes.pop() else {
            return;
        };
        self.current_scope = outer.scope;
        self.current_function = outer.function;
        self.is_global_star = outer.is_global_star;
        // Innermost first, since a scope can declare one name twice.
        let Self { shadowed, bindings, .. } = self;
        for (name, previous) in shadowed.drain(outer.shadowed_len..).rev() {
            if let Some(slot) = bindings.get_mut(name.index()) {
                *slot = previous;
            }
        }
    }

    fn visit_scoped_block(&mut self, kind: ScopeKind, block: &'ast Block<'ast>) {
        self.push_scope(kind);
        self.visit_block(block);
        self.pop_scope();
    }

    fn declare_local(&mut self, text: &'ast str, span: Span, kind: SymbolKind) -> Option<SymbolId> {
        let name = self.intern(text)?;
        let id = SymbolId::from_index(self.tree.symbols.len());
        self.tree.symbols.push(Symbol {
            id,
            name,
            scope: self.current_scope,
            kind,
            definition_span: span,
            shadows: self.binding(name).local(),
            is_upvalue: false,
            is_modified: false,
            is_exported: false,
            is_close: false,
        });
        self.tree.resolved_offsets.push(0);
        self.bind(name, Binding::Local(id));
        if self.tree.version.has_env_upvalue() && text == "_ENV" {
            self.env_name = Some(name);
        }
        Some(id)
    }

    /// `None` for a name the parser found missing.
    fn declare_name(&mut self, name: &Name<'ast>, kind: SymbolKind) -> Option<SymbolId> {
        if name.text.is_empty() {
            return None;
        }
        self.declare_local(name.text.as_str(), name.span, kind)
    }

    fn symbol_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.tree.symbols.get_mut(id.index())
    }

    /// Lua 5.5: a `global` declaration of `name`, which a value assigns when
    /// `is_assigned`.
    fn declare_global(&mut self, name: &Name<'ast>, is_assigned: bool) {
        if name.text.is_empty() {
            return;
        }
        let Some(id) = self.intern(name.text.as_str()) else {
            return;
        };
        if let Some(entry) = self.tree.names.get_mut(id.index()) {
            entry.is_declared_global = true;
            entry.is_assigned_global |= is_assigned;
        }
        self.bind(id, Binding::Global);
    }

    /// Records a use of `name`, resolved against the live declarations. A
    /// use from a nested function also marks the symbol it reads, or the
    /// local `_ENV` it indexes, as an upvalue, and a write marks a symbol
    /// modified.
    fn reference(&mut self, name: &Name<'ast>, kind: ReferenceKind) {
        if name.text.is_empty() {
            return;
        }
        let Some(name_id) = self.intern(name.text.as_str()) else {
            return;
        };
        let binding = self.binding(name_id);
        let resolved = binding.local();
        // Luau: `types` and the types in scope belong to the type runtime, so
        // inside a type function they're no references of this chunk.
        if resolved.is_none()
            && self.is_in_type_function
            && (name.text == "types" || self.type_names.contains(name.text.as_str()))
        {
            return;
        }
        let (env, is_declared_global) = match resolved {
            Some(_) => (None, false),
            None => (
                self.env_name.and_then(|env| self.binding(env).local()),
                matches!(binding, Binding::Global) || self.is_global_star,
            ),
        };
        if kind.is_write()
            && resolved.is_none()
            && env.is_none()
            && let Some(entry) = self.tree.names.get_mut(name_id.index())
        {
            entry.is_assigned_global = true;
        }
        let reference = ReferenceId::from_index(self.tree.references.len());
        self.tree.positions.push(Position { start: name.span.start, reference });
        match name.index.and_then(|index| self.tree.by_index.get_mut(offset_to_index(index.get())))
        {
            Some(slot @ None) => *slot = Some(reference),
            Some(Some(_)) | None => self.tree.is_indexed = false,
        }
        let ScopeTree { scopes, symbols, resolved_offsets, .. } = &mut self.tree;
        if let Some(symbol) = resolved.or(env).and_then(|symbol| symbols.get_mut(symbol.index())) {
            if scopes
                .get(symbol.scope.index())
                .is_some_and(|scope| scope.function != self.current_function)
            {
                symbol.is_upvalue = true;
            }
            if resolved.is_some() {
                symbol.is_modified |= kind.is_write();
                // Counted here, laid out by `finish`.
                if let Some(count) = resolved_offsets.get_mut(symbol.id.index().saturating_add(1)) {
                    *count = count.saturating_add(1);
                }
            }
        }
        self.tree.references.push(Reference {
            span: name.span,
            name: name_id,
            scope: self.current_scope,
            resolved,
            env,
            kind,
            is_declared_global,
        });
    }

    /// Luau: a type function sees every type of its block, earlier or later.
    fn record_type_names(&mut self, block: &'ast Block<'ast>) {
        for stmt in &block.stmts {
            if let Statement::TypeDeclaration(declaration) = stmt
                && self.filter.keeps(declaration.name.text.as_str())
            {
                self.type_names.insert(declaration.name.text.as_str());
            }
        }
    }

    /// An indexed or field target only reads its prefix and key.
    fn reference_var(&mut self, var: &'ast Var<'ast>, kind: ReferenceKind) {
        match var {
            Var::Name(name) => self.reference(name, kind),
            Var::Suffixed(chain) => {
                self.sink.chain(chain, true);
                self.walk_suffixed_expr(chain);
            }
        }
    }

    /// Values are read before targets are written, but targets come first in
    /// the source, so their positions move ahead of the values' to stay
    /// sorted.
    fn reorder_positions(&mut self, first_value: usize, first_target: usize) {
        if let Some(statement) = self.tree.positions.get_mut(first_value..) {
            statement.rotate_left(first_target.saturating_sub(first_value));
        }
    }

    /// Luau: the annotations of a function's signature. The parameters, and
    /// a `local function`'s own name, bind after the whole signature parses,
    /// so a `typeof` here reads the enclosing scope.
    fn visit_signature_annotations(&mut self, body: &'ast FunctionBody<'ast>) {
        for param in &body.params {
            if let Some(annotation) = &param.type_annotation {
                self.visit_type(annotation);
            }
        }
        if let Some(annotation) =
            body.vararg.as_ref().and_then(|vararg| vararg.type_annotation.as_ref())
        {
            self.visit_type(annotation);
        }
        if let Some(annotation) = &body.return_type {
            self.visit_type(annotation);
        }
    }

    /// Binds `body`'s parameters in a new function scope and visits its
    /// block.
    fn visit_function_scope(&mut self, body: &'ast FunctionBody<'ast>, is_method: bool) {
        self.sink.function_body(body);
        self.push_scope(ScopeKind::Function);
        if is_method {
            let span = Span::new(body.span.start, body.span.start);
            self.declare_local("self", span, SymbolKind::ImplicitSelf);
        }
        for param in &body.params {
            self.declare_name(&param.name, SymbolKind::Parameter);
        }
        // Lua 5.5: `...name` binds the vararg table to a local.
        if let Some(name) = body.vararg.as_ref().and_then(|vararg| vararg.name.as_ref()) {
            self.declare_name(name, SymbolKind::Parameter);
        }
        self.visit_block(&body.block);
        self.pop_scope();
    }

    fn visit_function_attributes(&mut self, attributes: &'ast [FunctionAttribute<'ast>]) {
        for attribute in attributes {
            self.visit_function_attribute(attribute);
        }
    }
}

impl<'ast, Sink, Filter> Visit<'ast> for Builder<'ast, '_, Sink, Filter>
where
    Sink: NodeSink<'ast>,
    Filter: NameFilter,
{
    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        self.sink.block(block);
        if self.tree.version.has_type_syntax() {
            self.record_type_names(block);
        }
        self.walk_block(block);
    }

    fn visit_function_body(&mut self, function_body: &'ast FunctionBody<'ast>) {
        self.visit_signature_annotations(function_body);
        self.visit_function_scope(function_body, false);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        self.sink.stmt(stmt);
        self.walk_stmt(stmt);
    }

    fn visit_assignment(&mut self, assignment: &'ast Assignment<'ast>) {
        let first_value = self.tree.positions.len();
        for expr in &assignment.values {
            self.visit_expr(expr);
        }
        let first_target = self.tree.positions.len();
        for var in &assignment.targets {
            self.reference_var(var, ReferenceKind::Write);
        }
        self.reorder_positions(first_value, first_target);
    }

    // Luau
    fn visit_compound_assignment(&mut self, compound_assignment: &'ast CompoundAssignment<'ast>) {
        let first_value = self.tree.positions.len();
        self.visit_expr(&compound_assignment.expr);
        let first_target = self.tree.positions.len();
        self.reference_var(&compound_assignment.var, ReferenceKind::ReadWrite);
        self.reorder_positions(first_value, first_target);
    }

    fn visit_local_assignment(&mut self, local_assignment: &'ast LocalAssignment<'ast>) {
        // Annotations and initializers can't see the names being declared.
        for attributed in &local_assignment.names {
            if let Some(annotation) = &attributed.type_annotation {
                self.visit_type(annotation);
            }
        }
        for expr in &local_assignment.exprs {
            self.visit_expr(expr);
        }
        let has_one_initializer_per_name =
            local_assignment.exprs.len() == local_assignment.names.len();
        for (i, attributed) in local_assignment.names.iter().enumerate() {
            let Some(id) = self.declare_name(&attributed.name, SymbolKind::Local) else {
                continue;
            };
            if has_one_initializer_per_name && let Some(initializer) = local_assignment.exprs.get(i)
            {
                self.initialized_locals.push((id, initializer));
            }
            let is_close = attributed
                .attribute
                .as_ref()
                .is_some_and(|attribute| attribute.name.text == "close");
            if let Some(symbol) = self.symbol_mut(id) {
                symbol.is_exported = local_assignment.kind.is_exported(); // Luau
                symbol.is_close = is_close;
            }
        }
    }

    fn visit_local_function(&mut self, local_function: &'ast LocalFunction<'ast>) {
        self.visit_function_attributes(&local_function.attributes);
        self.visit_signature_annotations(&local_function.body);
        // Declared before the body so the function can call itself.
        if let Some(id) = self.declare_name(&local_function.name, SymbolKind::FunctionName)
            && let Some(symbol) = self.symbol_mut(id)
        {
            symbol.is_exported = local_function.kind.is_exported(); // Luau
        }
        self.visit_function_scope(&local_function.body, false);
    }

    fn visit_function_decl(&mut self, function_decl: &'ast FunctionDecl<'ast>) {
        self.visit_function_attributes(&function_decl.attributes);
        // `function f()` writes `f`, but `function t.m()` and
        // `function t:m()` only read `t`. Recording a write would set off
        // unused and overwritten warnings on the usual module pattern
        // (`local M = {} function M.f() end return M`).
        if let Some(first) = function_decl.name.names.first() {
            let is_field_write =
                function_decl.name.names.len() > 1 || function_decl.name.method.is_some();
            let kind = if is_field_write { ReferenceKind::Read } else { ReferenceKind::Write };
            self.reference(first, kind);
        }
        self.visit_signature_annotations(&function_decl.body);
        self.visit_function_scope(&function_decl.body, function_decl.name.method.is_some());
    }

    fn visit_do_block(&mut self, do_block: &'ast DoBlock<'ast>) {
        self.visit_scoped_block(ScopeKind::Block, &do_block.block);
    }

    fn visit_while_loop(&mut self, while_loop: &'ast WhileLoop<'ast>) {
        self.visit_expr(&while_loop.condition);
        self.visit_scoped_block(ScopeKind::Loop, &while_loop.block);
    }

    fn visit_repeat_loop(&mut self, repeat_loop: &'ast RepeatLoop<'ast>) {
        self.push_scope(ScopeKind::Loop);
        self.visit_block(&repeat_loop.block);
        // The `until` condition can see the body's locals.
        self.visit_expr(&repeat_loop.condition);
        self.pop_scope();
    }

    fn visit_if_stmt(&mut self, if_stmt: &'ast IfStatement<'ast>) {
        self.visit_expr(&if_stmt.condition);
        self.visit_scoped_block(ScopeKind::Block, &if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.visit_expr(&clause.condition);
            self.visit_scoped_block(ScopeKind::Block, &clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.visit_scoped_block(ScopeKind::Block, &else_clause.block);
        }
    }

    fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor<'ast>) {
        if let Some(annotation) = &numeric_for.type_annotation {
            self.visit_type(annotation);
        }
        self.visit_expr(&numeric_for.start);
        self.visit_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.visit_expr(step);
        }
        self.push_scope(ScopeKind::Loop);
        self.declare_name(&numeric_for.name, SymbolKind::NumericForVariable);
        self.visit_block(&numeric_for.block);
        self.pop_scope();
    }

    fn visit_generic_for(&mut self, generic_for: &'ast GenericFor<'ast>) {
        // Annotations and iterator expressions can't see the loop variables.
        for binding in &generic_for.names {
            if let Some(annotation) = &binding.type_annotation {
                self.visit_type(annotation);
            }
        }
        for expr in &generic_for.exprs {
            self.visit_expr(expr);
        }
        self.push_scope(ScopeKind::Loop);
        for binding in &generic_for.names {
            self.declare_name(&binding.name, SymbolKind::IteratorVariable);
        }
        self.visit_block(&generic_for.block);
        self.pop_scope();
    }

    // Lua 5.5: the initializers can't see the names being declared, and a
    // name with an initializer is assigned.
    fn visit_global_declaration(&mut self, global_declaration: &'ast GlobalDeclaration<'ast>) {
        for expr in &global_declaration.exprs {
            self.visit_expr(expr);
        }
        let is_assigned = !global_declaration.exprs.is_empty();
        for attributed in &global_declaration.names {
            self.declare_global(&attributed.name, is_assigned);
        }
    }

    // Lua 5.5: declared before the body, so the body reads the global.
    fn visit_global_function(&mut self, global_function: &'ast GlobalFunction<'ast>) {
        self.declare_global(&global_function.name, true);
        self.visit_function_body(&global_function.body);
    }

    // Lua 5.5
    fn visit_global_star(&mut self, _global_star: &'ast GlobalStar<'ast>) {
        self.is_global_star = true;
    }

    // Luau: the body of a type function, unlike a type alias, runs in the
    // type runtime.
    fn visit_type_declaration(&mut self, type_declaration: &'ast TypeDeclaration<'ast>) {
        let is_enclosed_by_type_function = self.is_in_type_function;
        self.is_in_type_function |=
            matches!(type_declaration.value, TypeDeclarationValue::TypeFunction(_));
        self.walk_type_declaration(type_declaration);
        self.is_in_type_function = is_enclosed_by_type_function;
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        self.sink.expr(expr);
        if let Expression::Name(name) = expr {
            self.reference(name, ReferenceKind::Read);
        } else {
            self.walk_expr(expr);
        }
    }

    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        self.sink.chain(suffixed_expr, false);
        self.walk_suffixed_expr(suffixed_expr);
    }

    fn visit_table_constructor(&mut self, table_constructor: &'ast TableConstructor<'ast>) {
        self.sink.table(table_constructor);
        self.walk_table_constructor(table_constructor);
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_ast::{NameIndex, VisitMut};
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::parse;

    fn scope_tree_for<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        version: LuaVersion,
    ) -> ScopeTree<'ast> {
        ScopeTree::build(parse(allocator, source, version), TreeCounts::default())
    }

    fn scope_tree<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ScopeTree<'ast> {
        scope_tree_for(allocator, source, LuaVersion::Lua54)
    }

    fn symbol<'tree>(tree: &'tree ScopeTree<'_>, name: &str) -> &'tree Symbol {
        tree.symbols()
            .iter()
            .find(|symbol| tree.name(symbol.name) == name)
            .unwrap_or_else(|| panic!("{name} must be declared"))
    }

    fn references<'tree>(tree: &'tree ScopeTree<'_>, name: &str) -> Vec<&'tree Reference> {
        tree.references().iter().filter(|reference| tree.name(reference.name) == name).collect()
    }

    fn global_names<'ast>(tree: &ScopeTree<'ast>) -> Vec<&'ast str> {
        tree.global_references().map(|reference| tree.name(reference.name)).collect()
    }

    fn is_assigned_global(tree: &ScopeTree<'_>, name: &str) -> bool {
        tree.name_id(name).is_some_and(|id| tree.is_assigned_global(id))
    }

    fn is_declared_global(tree: &ScopeTree<'_>, name: &str) -> bool {
        tree.name_id(name).is_some_and(|id| tree.is_declared_global(id))
    }

    #[test]
    fn local_declaration_creates_symbol() {
        let allocator = Allocator::new();
        let tree = scope_tree(&allocator, "local x = 1");
        assert_eq!(tree.symbols().len(), 1);
        assert_eq!(tree.name(tree.symbols()[0].name), "x");
    }

    #[test]
    fn reference_resolves_to_local() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local x = 1
print(x)",
        );
        let x_references = references(&tree, "x");
        assert_eq!(x_references.len(), 1);
        assert_eq!(x_references[0].resolved, Some(symbol(&tree, "x").id));
    }

    #[test]
    fn global_reference_is_unresolved() {
        let allocator = Allocator::new();
        let tree = scope_tree(&allocator, "print(hello)");
        let names = global_names(&tree);
        assert!(names.contains(&"print"));
        assert!(names.contains(&"hello"));
    }

    #[test]
    fn function_body_creates_scope() {
        let allocator = Allocator::new();
        let tree = scope_tree(&allocator, "local function f(a, b) return a + b end");
        assert_eq!(tree.scopes().len(), 2);
        assert_eq!(tree.scopes()[1].kind, ScopeKind::Function);
        assert_eq!(tree.symbols().len(), 3);
        assert_eq!(symbol(&tree, "a").scope, ScopeId::from_index(1));
    }

    #[test]
    fn local_read_from_nested_function_is_upvalue() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local x = 1
local function f() return x end",
        );
        let x_symbol = &tree.symbols()[0];
        assert_eq!(tree.name(x_symbol.name), "x");
        assert!(x_symbol.is_upvalue);
    }

    #[test]
    fn local_read_two_functions_deep_is_upvalue() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local x = 1
local function outer()
  local function inner() return x end
end",
        );
        assert!(
            symbol(&tree, "x").is_upvalue,
            "x should be detected as upvalue when captured by nested inner function"
        );
    }

    #[test]
    fn local_read_from_a_nested_block_is_no_upvalue() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local function f(a)
  do while a do print(a) end end
end",
        );
        assert!(!symbol(&tree, "a").is_upvalue);
    }

    #[test]
    fn inner_local_shadows_outer_local() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local x = 1
do
  local x = 2
end",
        );
        assert_eq!(tree.symbols().len(), 2);
        let inner_x = &tree.symbols()[1];
        assert_eq!(tree.name(inner_x.name), "x");
        assert_eq!(inner_x.shadows, Some(tree.symbols()[0].id));
    }

    #[test]
    fn redeclaring_a_local_in_one_scope_restores_the_outer_one_on_exit() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local x = 1
do
  local x = 2
  local x = 3
end
print(x)",
        );
        let last_reference = *references(&tree, "x").last().expect("print(x) references x");
        assert_eq!(last_reference.resolved, Some(tree.symbols()[0].id));
        assert_eq!(tree.symbols()[2].shadows, Some(tree.symbols()[1].id));
    }

    #[test]
    fn numeric_for_declares_loop_variable() {
        let allocator = Allocator::new();
        let tree = scope_tree(&allocator, "for i = 1, 10 do print(i) end");
        assert_eq!(symbol(&tree, "i").kind, SymbolKind::NumericForVariable);
    }

    #[test]
    fn generic_for_declares_iterator_variables() {
        let allocator = Allocator::new();
        let tree = scope_tree(&allocator, "for k, v in pairs(t) do print(k, v) end");
        let iterator_variable_count = tree
            .symbols()
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::IteratorVariable)
            .count();
        assert_eq!(iterator_variable_count, 2);
    }

    #[test]
    fn numeric_for_variable_not_visible_after_loop() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
for i = 1, 10 do end
print(i)",
        );
        assert!(
            references(&tree, "i").iter().any(|reference| reference.resolved.is_none()),
            "for-loop variable i should not be visible outside the loop"
        );
    }

    #[test]
    fn generic_for_variables_not_visible_after_loop() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
for k, v in pairs(t) do end
print(k)",
        );
        assert!(
            references(&tree, "k").iter().any(|reference| reference.resolved.is_none()),
            "generic for variables should not be visible outside the loop"
        );
    }

    #[test]
    fn named_vararg_declares_symbol() {
        // Lua 5.5: `...name` binds the vararg table to a name.
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "local function f(...args) return args end",
            LuaVersion::Lua55,
        );
        let args_symbol = symbol(&tree, "args");
        assert_eq!(args_symbol.kind, SymbolKind::Parameter);
        let args_references = references(&tree, "args");
        assert_eq!(args_references.len(), 1);
        assert!(
            args_references[0].resolved == Some(args_symbol.id),
            "reference must resolve to the vararg binding"
        );
    }

    #[test]
    fn method_declares_self() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local t = {}
function t:method() return self end",
        );
        assert!(
            tree.symbols().iter().any(|symbol| tree.name(symbol.name) == "self"),
            "self should be declared as a symbol in method scope"
        );
    }

    #[test]
    fn dot_function_declares_no_self() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local t = {}
function t.method() end",
        );
        assert!(
            tree.name_id("self").is_none(),
            "self should not be declared in non-method function"
        );
    }

    #[test]
    fn repeat_until_condition_sees_block_locals() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
repeat
  local done = true
until done",
        );
        let done_references = references(&tree, "done");
        assert_eq!(done_references.len(), 1);
        assert!(
            done_references[0].resolved.is_some(),
            "repeat-until condition should resolve locals from the loop body"
        );
    }

    #[test]
    fn global_function_body_captures_enclosing_local() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local x = 1
global function f() return x end",
            LuaVersion::Lua55,
        );
        assert!(symbol(&tree, "x").is_upvalue);
    }

    #[test]
    fn global_stmts_record_their_declared_names() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
global a, b <const> = 1, 2
global c
global function d() end
",
            LuaVersion::Lua55,
        );
        for name in ["a", "b", "c", "d"] {
            assert!(is_declared_global(&tree, name), "{name}");
        }
    }

    #[test]
    fn local_env_makes_free_names_its_fields() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local _ENV = {}
x = 1
print(y)",
        );
        let env_id = symbol(&tree, "_ENV").id;
        assert_eq!(references(&tree, "y")[0].env, Some(env_id));
        assert_eq!(tree.global_references().count(), 0);
        assert!(!is_assigned_global(&tree, "x"), "`x = 1` writes the local table");
    }

    #[test]
    fn local_env_is_an_ordinary_local_before_52() {
        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            let allocator = Allocator::new();
            let tree = scope_tree_for(
                &allocator,
                "\
local _ENV = {}
print(y)",
                version,
            );
            assert_eq!(references(&tree, "y")[0].env, None, "{version:?}");
            assert_eq!(tree.global_references().count(), 2, "{version:?}");
            assert!(!tree.is_env_binding(symbol(&tree, "_ENV").id), "{version:?}");
        }
    }

    #[test]
    fn local_env_stops_redirecting_free_names_with_its_block() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
do local _ENV = {} end
print(y)",
        );
        assert_eq!(references(&tree, "y")[0].env, None);
    }

    #[test]
    fn assigning_the_chunk_env_redirects_every_free_name() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
_ENV = {}
print(y)",
        );
        assert!(tree.is_env_assigned());
        assert_eq!(tree.global_references().count(), 0);

        let lua51 = scope_tree_for(
            &allocator,
            "\
_ENV = {}
print(y)",
            LuaVersion::Lua51,
        );
        assert!(!lua51.is_env_assigned());
        assert_eq!(lua51.global_references().count(), 3);
    }

    #[test]
    fn assigned_globals_records_free_name_writes_only() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
x = 1
function f() end
function t.m() end
local y
y = 2",
        );
        assert!(is_assigned_global(&tree, "x"));
        assert!(is_assigned_global(&tree, "f"));
        assert!(!is_assigned_global(&tree, "t"), "`t.m` only reads `t`");
        assert!(!is_assigned_global(&tree, "y"), "`y` is a local");
    }

    #[test]
    fn global_declaration_ends_with_its_block() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local x = 1
do global x end
print(x)",
            LuaVersion::Lua55,
        );
        let last_reference = *references(&tree, "x").last().expect("print(x) references x");
        assert_eq!(last_reference.resolved, Some(symbol(&tree, "x").id));
    }

    #[test]
    fn global_declaration_initializer_reads_the_enclosing_local() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local x = 1
global x = x
print(x)",
            LuaVersion::Lua55,
        );
        let x_references = references(&tree, "x");
        assert_eq!(x_references[0].resolved, Some(symbol(&tree, "x").id));
        assert_eq!(x_references[1].resolved, None);
        assert!(is_assigned_global(&tree, "x"));
    }

    #[test]
    fn declared_global_without_initializer_is_not_assigned() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
global x
global *
print(y)",
            LuaVersion::Lua55,
        );
        assert!(["x", "y", "print"].iter().all(|name| !is_assigned_global(&tree, name)));
    }

    #[test]
    fn global_star_covers_free_names_until_its_block_ends() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
do global *
a = 1 end
b = 2",
            LuaVersion::Lua55,
        );
        assert!(references(&tree, "a")[0].is_declared_global);
        assert!(!references(&tree, "b")[0].is_declared_global);
    }

    #[test]
    fn global_star_does_not_void_enclosing_locals() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local x = 1
global *
print(x)",
            LuaVersion::Lua55,
        );
        assert_eq!(references(&tree, "x")[0].resolved, Some(symbol(&tree, "x").id));
    }

    #[test]
    fn explicit_self_stays_a_parameter() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local t = {}
function t.m(self) return self end",
        );
        assert_eq!(symbol(&tree, "self").kind, SymbolKind::Parameter);
    }

    #[test]
    fn method_self_is_implicit_with_an_empty_span() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local t = {}
function t:m() return self end",
        );
        let self_symbol = symbol(&tree, "self");
        assert_eq!(self_symbol.kind, SymbolKind::ImplicitSelf);
        assert_eq!(self_symbol.definition_span.start, self_symbol.definition_span.end);
    }

    #[test]
    fn type_function_body_sees_a_later_type() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
type function f(t) return g(t) end
type function g(t) return t end",
            LuaVersion::Luau,
        );
        assert_eq!(references(&tree, "g").len(), 0);
    }

    #[test]
    fn type_runtime_names_are_only_known_in_type_functions() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
type T = number
print(types, T)",
            LuaVersion::Luau,
        );
        assert_eq!(tree.global_references().count(), 3);
    }

    #[test]
    fn reference_of_finds_each_reference_from_its_name() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local x = 1
print(x)
t[x], x = x, function() y = x end",
        );
        for (i, reference) in tree.references().iter().enumerate() {
            let name = Name::new(tree.name(reference.name).into(), None, reference.span);
            assert_eq!(tree.reference_of(&name), Some(ReferenceId::from_index(i)));
        }
    }

    /// Every name, those that make references and those that make none.
    struct AllNames<'ast>(Vec<&'ast Name<'ast>>);

    impl<'ast> Visit<'ast> for AllNames<'ast> {
        fn visit_name(&mut self, name: &'ast Name<'ast>) {
            self.0.push(name);
        }
    }

    /// Each reference a tree built from `counts` finds from the names of
    /// `chunk`, and then each one a tree that searches positions finds.
    fn found_by_index_and_by_search(
        chunk: &Chunk<'_>,
        counts: TreeCounts,
    ) -> (Vec<Option<ReferenceId>>, Vec<Option<ReferenceId>>) {
        let indexed = ScopeTree::build(chunk, counts);
        let searched = ScopeTree::build(chunk, TreeCounts::default());
        let mut names = AllNames(Vec::new());
        names.visit_chunk(chunk);
        (
            names.0.iter().map(|name| indexed.reference_of(name)).collect(),
            names.0.iter().map(|name| searched.reference_of(name)).collect(),
        )
    }

    const NAMES_OF_EVERY_KIND: &str = "\
local x, y = 1
print(x, t.x)
t[x], x = x, function(x) y = x.y end";

    #[test]
    fn a_parsed_tree_finds_a_name_s_reference_by_its_index() {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, NAMES_OF_EVERY_KIND, LuaVersion::Lua54);
        let (by_index, by_search) = found_by_index_and_by_search(&parsed.chunk, parsed.counts);
        assert_eq!(by_index, by_search);
        assert_eq!(by_index.iter().flatten().count(), 9, "{by_index:?}");
    }

    #[test]
    fn a_tree_whose_names_share_an_index_searches_for_their_references() {
        struct Renumber;

        impl<'ast> VisitMut<'ast> for Renumber {
            fn visit_name(&mut self, name: &mut Name<'ast>) {
                name.index = Some(NameIndex::new(0));
            }
        }

        let allocator = Allocator::new();
        let mut parsed = luck_parser::parse(&allocator, NAMES_OF_EVERY_KIND, LuaVersion::Lua54);
        Renumber.visit_block(&mut parsed.chunk.block);
        let (by_index, by_search) = found_by_index_and_by_search(&parsed.chunk, parsed.counts);
        assert_eq!(by_index, by_search);
        assert_eq!(by_index.iter().flatten().count(), 9, "{by_index:?}");
    }

    #[test]
    fn references_within_lists_a_span_s_names_in_source_order() {
        let allocator = Allocator::new();
        let source = "\
local x = 1
t[x], x = y, function() z = x end
print(x)";
        let tree = scope_tree(&allocator, source);
        let line = Span::new(12, 45);
        let names: Vec<&str> =
            tree.references_within(line).map(|id| tree.name(tree.reference(id).name)).collect();
        assert_eq!(names, ["t", "x", "x", "y", "z", "x"]);
        assert_eq!(tree.references_within(Span::new(13, 15)).count(), 1, "only `x`, not `t`");
    }

    #[test]
    fn assignment_values_are_read_before_targets_are_written() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local x = 1
x = x + 1",
        );
        let kinds: Vec<ReferenceKind> = tree
            .resolved_references(symbol(&tree, "x").id)
            .iter()
            .map(|&id| tree.reference(id).kind)
            .collect();
        assert_eq!(kinds, [ReferenceKind::Read, ReferenceKind::Write]);
    }

    #[test]
    fn resolved_references_lists_each_symbols_own_in_order() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local a, b = 1, 2
print(b, a, b)
local c",
        );
        let spans = |name: &str| -> Vec<u32> {
            tree.resolved_references(symbol(&tree, name).id)
                .iter()
                .map(|&id| tree.reference(id).span.start)
                .collect()
        };
        assert_eq!(spans("a"), [27]);
        assert_eq!(spans("b"), [24, 30]);
        assert_eq!(spans("c"), Vec::<u32>::new());
    }

    #[test]
    fn resolves_to_local_checks_the_name_at_the_span() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local x = 1
print(x)
print(y)",
        );
        let local_x = references(&tree, "x")[0].span;
        let global_y = references(&tree, "y")[0].span;
        assert!(tree.resolves_to_local(&Name::new("x".into(), None, local_x)));
        assert!(!tree.resolves_to_local(&Name::new("y".into(), None, local_x)));
        assert!(!tree.resolves_to_local(&Name::new("y".into(), None, global_y)));
    }

    #[test]
    fn type_alias_typeof_reads_enclosing_local() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local x = 1
type T = typeof(x)",
            LuaVersion::Luau,
        );
        let x_references = references(&tree, "x");
        assert_eq!(x_references.len(), 1);
        assert_eq!(x_references[0].resolved, Some(symbol(&tree, "x").id));
    }

    #[test]
    fn implicit_self_span_does_not_cover_the_method_body() {
        let allocator = Allocator::new();
        let tree = scope_tree(
            &allocator,
            "\
local t = {}
function t:m()
  local x = 1
  return x
end",
        );
        let self_span = symbol(&tree, "self").definition_span;
        let x_span = symbol(&tree, "x").definition_span;
        assert!(
            self_span.end <= x_span.start || x_span.end <= self_span.start,
            "implicit self must not claim the span of the declarations in its method body, \
             or a cursor on any of them resolves to self: self {self_span:?}, x {x_span:?}"
        );
    }

    #[test]
    fn global_declaration_shadows_enclosing_local() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local x = 1
global print, x
print(x)",
            LuaVersion::Lua55,
        );
        assert_eq!(
            references(&tree, "x")[0].resolved,
            None,
            "after `global x`, the name x refers to the global and voids the enclosing local"
        );
    }

    #[test]
    fn global_function_shadows_enclosing_local() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local f = 1
global function f() end
f()",
            LuaVersion::Lua55,
        );
        let last_reference = *references(&tree, "f").last().expect("f() references f");
        assert_eq!(
            last_reference.resolved, None,
            "after `global function f`, the name f refers to the global and voids the enclosing local"
        );
    }

    #[test]
    fn global_function_body_refers_to_the_global() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local f = 1
global function f() return f end",
            LuaVersion::Lua55,
        );
        assert!(
            !symbol(&tree, "f").is_upvalue,
            "the body of `global function f` sees the global f, so it captures no local f"
        );
    }

    #[test]
    fn param_annotation_typeof_does_not_see_parameters() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local a = 1
local function f(a: number, b: typeof(a)) return b end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "a")[0].resolved,
            Some(symbol(&tree, "a").id),
            "Luau parses parameter annotations before binding any parameter, so typeof(a) \
             names the outer local"
        );
    }

    #[test]
    fn return_annotation_typeof_does_not_see_parameters() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local a = 1
local function f(a: number): typeof(a) return a end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "a")[0].resolved,
            Some(symbol(&tree, "a").id),
            "Luau parses the return annotation before binding the parameters, so typeof(a) \
             names the outer local"
        );
    }

    #[test]
    fn local_annotation_typeof_does_not_see_sibling_name() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local a = 1
do
  local a, b: typeof(a) = 2, 3
  print(a, b)
end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "a")[0].resolved,
            Some(symbol(&tree, "a").id),
            "the names of a local statement bind after the statement, so typeof(a) in its \
             annotation names the outer local"
        );
    }

    #[test]
    fn numeric_for_annotation_typeof_does_not_see_loop_variable() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local i = 1
for i: typeof(i) = 1, 2 do print(i) end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "i")[0].resolved,
            Some(symbol(&tree, "i").id),
            "the loop variable binds after the loop header, so typeof(i) in its annotation \
             names the outer local"
        );
    }

    #[test]
    fn generic_for_annotation_typeof_does_not_see_loop_variables() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local k = 1
for k, v: typeof(k) in pairs({}) do print(k, v) end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "k")[0].resolved,
            Some(symbol(&tree, "k").id),
            "the loop variables bind after the loop header, so typeof(k) in an annotation \
             names the outer local"
        );
    }

    #[test]
    fn local_function_annotation_typeof_does_not_see_its_name() {
        let allocator = Allocator::new();
        let tree = scope_tree_for(
            &allocator,
            "\
local g = 1
local function g(z: typeof(g)) return z end",
            LuaVersion::Luau,
        );
        assert_eq!(
            references(&tree, "g")[0].resolved,
            Some(symbol(&tree, "g").id),
            "Luau binds a local function's name after parsing its signature, so typeof(g) \
             in a parameter annotation names the outer local"
        );
    }
}
