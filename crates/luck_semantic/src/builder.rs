use std::mem;

use luck_ast::{
    Assignment, AstKind, Block, Call, CompoundAssignment, ConditionalBinding, DoBlock, Expression,
    FunctionBody, FunctionDecl, FunctionDef, GenericFor, GetSpan as _, IfExpression, IfStatement,
    LocalAssignment, LocalFunction, LocalFunctionKind, Name, NumericFor, RepeatLoop, Statement,
    SuffixedExpression, TreeCounts, Type, TypeDeclaration, TypeDeclarationValue, TypedName, Var,
    Visit, WhileLoop,
};
use luck_diagnostics::Diagnostic;
use luck_token::Span;

use crate::checker::{
    assignment_to_read_only, type_function_outer_local, until_reads_skipped_local,
};
use crate::id::{NameId, NodeId, ReferenceId, ScopeId, SymbolId};
use crate::node::AstNodes;
use crate::require_site::{require_call, require_sites};
use crate::scoping::{
    NameRole, Reference, ReferenceKind, Scope, ScopeKind, Scoping, Symbol, SymbolKind,
    distinct_names_estimate,
};
use crate::semantic::Semantic;

/// One walk over a chunk, in source order, that builds its [`Semantic`].
///
/// It always resolves every name into a [`Scoping`]. Each other part is
/// built only when asked for: the checker's errors, the node table, and the
/// `require` sites.
///
/// The walk recurses once per level of nesting.
#[derive(Debug, Clone, Copy, Default)]
pub struct SemanticBuilder {
    counts: TreeCounts,
    is_checking: bool,
    is_building_nodes: bool,
    is_finding_requires: bool,
}

/// What [`SemanticBuilder::build`] returns.
#[derive(Debug)]
pub struct SemanticBuilderReturn<'ast> {
    pub semantic: Semantic<'ast>,
    /// The checker's errors, in source order, each an E008. Empty unless
    /// [`SemanticBuilder::with_check`] asked for them.
    pub errors: Vec<Diagnostic>,
}

impl SemanticBuilder {
    /// A build of the scoping alone.
    pub const fn new() -> Self {
        Self {
            counts: TreeCounts { blocks: 0, declarations: 0, name_uses: 0, names: 0 },
            is_checking: false,
            is_building_nodes: false,
            is_finding_requires: false,
        }
    }

    /// Sizes every table from `counts`, the parse's, so none of them grows.
    #[must_use]
    pub const fn with_counts(mut self, counts: TreeCounts) -> Self {
        self.counts = counts;
        self
    }

    /// Reports the errors Luau rejects a chunk for beyond its grammar: a
    /// write to a read-only binding, a type function that reads a local from
    /// outside it, and an `until` that reads a local a `continue` skips.
    ///
    /// On a tree that parsed with errors, they may only echo them.
    #[must_use]
    pub const fn with_check(mut self, is_checking: bool) -> Self {
        self.is_checking = is_checking;
        self
    }

    /// Builds the [`AstNodes`] table.
    #[must_use]
    pub const fn with_nodes(mut self, is_building_nodes: bool) -> Self {
        self.is_building_nodes = is_building_nodes;
        self
    }

    /// Finds the [`RequireSite`](crate::RequireSite)s.
    #[must_use]
    pub const fn with_requires(mut self, is_finding_requires: bool) -> Self {
        self.is_finding_requires = is_finding_requires;
        self
    }

    pub fn build<'ast>(self, chunk: &'ast Block<'ast>) -> SemanticBuilderReturn<'ast> {
        self.build_into(chunk, Scoping::with_capacity(self.counts))
    }

    /// [`Self::build`], into the tables of `scoping`, an earlier build's,
    /// which it empties first, so a pass that analyzes a tree again, as one
    /// that rewrites it does, reuses their memory.
    pub fn rebuild<'ast>(
        self,
        chunk: &'ast Block<'ast>,
        mut scoping: Scoping,
    ) -> SemanticBuilderReturn<'ast> {
        scoping.reset(self.counts);
        self.build_into(chunk, scoping)
    }

    fn build_into<'ast>(
        self,
        chunk: &'ast Block<'ast>,
        scoping: Scoping,
    ) -> SemanticBuilderReturn<'ast> {
        let counts = self.counts;
        let names = counts.names as usize;
        let module = ScopeId::from_index(0);
        let mut builder = Builder {
            is_checking: self.is_checking,
            scoping,
            // A chunk has about four nodes for each name.
            nodes: self.is_building_nodes.then(|| AstNodes::with_capacity(names * 4)),
            current_node: None,
            next_node: NodeId::from_index(0),
            require_calls: self.is_finding_requires.then(Vec::new),
            errors: Vec::new(),
            bindings: Vec::with_capacity(distinct_names_estimate(counts)),
            current_scope: module,
            current_function: module,
            outer_scopes: Vec::new(),
            shadowed: Vec::new(),
            loops: Vec::new(),
            loop_base: 0,
            until: None,
            type_function_scope: None,
            type_depth: 0,
            local_functions: Vec::new(),
            type_names: Vec::new(),
            scratch_nodes: Vec::new(),
            deferred_writes: Vec::new(),
        };
        builder.chunk(chunk);
        builder.finish()
    }
}

/// What a scope's exit restores.
struct OuterScope {
    scope: ScopeId,
    function: ScopeId,
    shadowed_len: usize,
    type_names_len: usize,
}

/// A loop a `continue` goes on with.
enum LoopFrame {
    WhileOrFor,
    Repeat {
        /// The first symbol its body declares after its first `continue`.
        first_skipped: Option<SymbolId>,
    },
}

/// An `until` condition whose loop body declares locals after a `continue`,
/// which the condition may not read.
struct UntilCheck {
    scope: ScopeId,
    first_skipped: SymbolId,
    condition: Span,
    reported: Vec<SymbolId>,
}

/// What a function binds besides its parameters.
#[derive(Clone, Copy)]
enum FunctionBinding<'ast> {
    Nothing,
    /// A `local function`'s own name, which binds after its signature and
    /// before its body.
    OwnName {
        name: &'ast Name<'ast>,
        declaration: NodeId,
        kind: LocalFunctionKind,
    },
    /// A method's implicit `self`, which binds before its parameters.
    ImplicitSelf,
}

struct Builder<'ast> {
    is_checking: bool,
    scoping: Scoping,
    nodes: Option<AstNodes<'ast>>,
    /// The innermost node the walk is in, while it builds the table. Leaving
    /// a node moves this to the parent the table holds for it.
    current_node: Option<NodeId>,
    next_node: NodeId,
    /// Each [`require_call`], with the reference its prefix makes.
    require_calls: Option<Vec<(Call<'ast>, ReferenceId)>>,
    errors: Vec<Diagnostic>,
    /// Each name's innermost live local, by name, `None` for a free name,
    /// as far as the last name a declaration binds. Resolving a name is one
    /// hash of it, and leaving a scope hashes nothing.
    bindings: Vec<Option<SymbolId>>,
    // The scope stack is never empty, so its top lives here and reading it
    // doesn't branch.
    current_scope: ScopeId,
    current_function: ScopeId,
    outer_scopes: Vec<OuterScope>,
    /// Undo log: the binding each declaration since the outermost open
    /// scope's entry replaced, but for the chunk's scope, which never closes.
    shadowed: Vec<(NameId, Option<SymbolId>)>,
    /// The open loops, of every function, and where the current function's
    /// start.
    loops: Vec<LoopFrame>,
    loop_base: usize,
    until: Option<UntilCheck>,
    /// The first scope of the innermost type function, so every scope before
    /// it is outside it.
    type_function_scope: Option<ScopeId>,
    /// How many types the walk is in. Types never run, so a reference in one
    /// captures nothing.
    type_depth: u32,
    /// The `local function`s whose bodies the walk is in, innermost last.
    local_functions: Vec<SymbolId>,
    /// Every type the open blocks declare, outermost block's first.
    type_names: Vec<&'ast str>,
    /// The nodes of names a statement binds once its values are read,
    /// innermost statement's last.
    scratch_nodes: Vec<NodeId>,
    /// The node of each name an assignment writes once its values are read,
    /// innermost assignment's last.
    deferred_writes: Vec<NodeId>,
}

impl<'ast> Builder<'ast> {
    fn finish(mut self) -> SemanticBuilderReturn<'ast> {
        self.scoping.finish();
        let requires = require_sites(self.require_calls.unwrap_or_default(), &self.scoping);
        self.errors.sort_by_key(|error| error.span.map(|span| span.start));
        SemanticBuilderReturn {
            semantic: Semantic::new(self.scoping, self.nodes.unwrap_or_default(), requires),
            errors: self.errors,
        }
    }

    /// Opens the node `kind` refers to, which the walk is in until
    /// [`Self::leave`].
    #[inline]
    fn enter(&mut self, kind: AstKind<'ast>) -> NodeId {
        let id = self.next_node;
        self.next_node = id.next();
        if let Some(nodes) = &mut self.nodes {
            nodes.push(id, kind, self.current_scope, self.current_node);
            self.current_node = Some(id);
        }
        id
    }

    #[inline]
    fn leave(&mut self) {
        if let Some(nodes) = &self.nodes
            && let Some(node) = self.current_node
        {
            self.current_node = nodes.parent_id(node);
        }
    }

    /// A name's node, which makes no reference.
    fn name_node(&mut self, name: &'ast Name<'ast>) -> NodeId {
        let node = self.enter(AstKind::Name(name));
        self.leave();
        node
    }

    /// The node of a name a statement declares, which holds its annotation.
    fn typed_name_node(&mut self, typed_name: &'ast TypedName<'ast>) -> NodeId {
        let node = self.enter(AstKind::TypedName(typed_name));
        self.name_node(&typed_name.name);
        if let Some(annotation) = &typed_name.type_annotation {
            self.visit_type(annotation);
        }
        self.leave();
        node
    }

    /// The nodes of names a statement declares, kept in the scratch nodes
    /// from the index it returns until [`Self::declare_typed_names`] binds
    /// them.
    fn typed_name_nodes(&mut self, typed_names: &'ast [TypedName<'ast>]) -> usize {
        let first = self.scratch_nodes.len();
        for typed_name in typed_names {
            let node = self.typed_name_node(typed_name);
            self.scratch_nodes.push(node);
        }
        first
    }

    /// Binds `typed_names` in the current scope, each to the node
    /// [`Self::typed_name_nodes`] kept for it from `first`.
    fn declare_typed_names(
        &mut self,
        typed_names: &[TypedName<'_>],
        first: usize,
        kind: SymbolKind,
        is_exported: bool,
        is_read_only: bool,
    ) {
        for (i, typed_name) in typed_names.iter().enumerate() {
            let node = self.scratch_nodes[first + i];
            if let Some(symbol) = self.declare_name(&typed_name.name, kind, node) {
                symbol.is_exported = is_exported;
                symbol.is_read_only = is_read_only;
            }
        }
        self.scratch_nodes.truncate(first);
    }

    fn push_scope(&mut self, kind: ScopeKind, node: NodeId) {
        let id = self.scoping.next_scope_id();
        let function = if kind == ScopeKind::Function { id } else { self.current_function };
        self.scoping.add_scope(Scope { parent: Some(self.current_scope), kind, function, node });
        self.outer_scopes.push(OuterScope {
            scope: mem::replace(&mut self.current_scope, id),
            function: mem::replace(&mut self.current_function, function),
            shadowed_len: self.shadowed.len(),
            type_names_len: self.type_names.len(),
        });
    }

    fn pop_scope(&mut self) {
        let outer = self.outer_scopes.pop().expect("a scope's exit follows its entry");
        self.current_scope = outer.scope;
        self.current_function = outer.function;
        // Innermost first, since a scope can declare one name twice.
        for (name, previous) in self.shadowed.drain(outer.shadowed_len..).rev() {
            self.bindings[name.index()] = previous;
        }
        self.type_names.truncate(outer.type_names_len);
    }

    fn declare(
        &mut self,
        text: &str,
        span: Span,
        kind: SymbolKind,
        declaration: NodeId,
    ) -> SymbolId {
        let name = self.scoping.intern(text);
        if self.bindings.len() <= name.index() {
            self.bindings.resize(name.index() + 1, None);
        }
        let shadows = self.bindings[name.index()];
        let id =
            self.scoping.add_symbol(name, self.current_scope, kind, declaration, span, shadows);
        self.bindings[name.index()] = Some(id);
        // The chunk's scope never closes, so its bindings need no undoing.
        if !self.outer_scopes.is_empty() {
            self.shadowed.push((name, shadows));
        }
        id
    }

    /// `None` for a name the parser found missing.
    fn declare_name(
        &mut self,
        name: &Name<'_>,
        kind: SymbolKind,
        declaration: NodeId,
    ) -> Option<&mut Symbol> {
        if name.text.is_empty() {
            return None;
        }
        let id = self.declare(name.text.as_str(), name.span, kind, declaration);
        self.scoping.set_role(name.index, NameRole::symbol(id));
        Some(self.scoping.symbol_mut(id))
    }

    /// Records a use of `name`, whose node is `node`, resolved against the
    /// live declarations. A use from a nested function also marks the symbol
    /// it reads as an upvalue, and a write marks it modified.
    fn reference(
        &mut self,
        name: &Name<'_>,
        kind: ReferenceKind,
        node: NodeId,
    ) -> Option<ReferenceId> {
        let text = name.text.as_str();
        if text.is_empty() {
            return None;
        }
        let name_id = self.scoping.intern(text);
        let resolved = self.bindings.get(name_id.index()).copied().flatten();
        let mut is_recursive = false;
        if let Some(symbol_id) = resolved {
            if self.is_checking {
                self.check_reference(name, kind, symbol_id);
            }
            let symbol = self.scoping.symbol_mut(symbol_id);
            // A live local's scope is open, and every open scope from the
            // current function's on is that function's, so one before it is
            // an enclosing function's.
            symbol.is_upvalue |= self.type_depth == 0 && symbol.scope < self.current_function;
            symbol.is_modified |= kind.is_write();
            // Only a `local function`'s own name has a body the walk can be in.
            is_recursive = symbol.kind == SymbolKind::FunctionName
                && self.local_functions.contains(&symbol_id);
        } else if self.type_function_scope.is_some()
            && (text == "types" || self.type_names.contains(&text))
        {
            // `types` and the types in scope belong to the type runtime, so
            // inside a type function they're no references of this chunk.
            return None;
        } else if kind.is_write() {
            self.scoping.mark_assigned_global(name_id);
        }
        let reference = Reference {
            span: name.span,
            name: name_id,
            scope: self.current_scope,
            node,
            resolved,
            kind,
            is_in_type: self.type_depth > 0,
            is_recursive,
        };
        let id = self.scoping.add_reference(reference);
        self.scoping.set_role(name.index, NameRole::reference(id));
        Some(id)
    }

    /// A name's node and the reference it makes.
    fn reference_name(
        &mut self,
        name: &'ast Name<'ast>,
        kind: ReferenceKind,
    ) -> Option<ReferenceId> {
        let node = self.enter(AstKind::Name(name));
        let reference = self.reference(name, kind, node);
        self.leave();
        reference
    }

    /// The checker's errors for a use of `name`, which resolves to the local
    /// `id`.
    fn check_reference(&mut self, name: &Name<'_>, kind: ReferenceKind, id: SymbolId) {
        let text = name.text.as_str();
        let &Symbol { scope, is_read_only, .. } = self.scoping.symbol(id);
        if self.type_function_scope.is_some_and(|first| scope < first) {
            self.errors.push(type_function_outer_local(name.span, text));
        }
        if let Some(until) = &mut self.until
            && self.type_depth == 0
            && until.scope == scope
            && until.first_skipped <= id
            && !until.reported.contains(&id)
        {
            until.reported.push(id);
            self.errors.push(until_reads_skipped_local(until.condition, text));
        }
        if kind.is_write() && is_read_only {
            self.errors.push(assignment_to_read_only(name.span, text));
        }
    }

    /// A type function sees every type of its block, earlier or later.
    fn record_type_names(&mut self, block: &'ast Block<'ast>) {
        for stmt in &block.stmts {
            if let Statement::TypeDeclaration(declaration) = stmt {
                self.type_names.push(declaration.name.text.as_str());
            }
        }
    }

    fn bound_expr(
        &mut self,
        opener: NodeId,
        binding: Option<(&'ast ConditionalBinding<'ast>, NodeId)>,
        expr: &'ast Expression<'ast>,
    ) {
        if binding.is_none() {
            self.visit_expr(expr);
            return;
        }
        self.push_scope(ScopeKind::Block, opener);
        self.declare_binding(binding);
        self.visit_expr(expr);
        self.pop_scope();
    }

    /// The statements of a block whose scope is open.
    fn walk_stmts(&mut self, block: &'ast Block<'ast>) {
        self.record_type_names(block);
        for stmt in &block.stmts {
            if matches!(stmt, Statement::Continue(_)) {
                self.mark_continue();
            }
            self.visit_stmt(stmt);
        }
    }

    /// The chunk's block, which opens the module's scope.
    fn chunk(&mut self, chunk: &'ast Block<'ast>) {
        let node = self.enter(AstKind::Block(chunk));
        let module = self.current_scope;
        self.scoping.add_scope(Scope {
            parent: None,
            kind: ScopeKind::Module,
            function: module,
            node,
        });
        self.walk_stmts(chunk);
        self.leave();
    }

    /// A block whose scope its parent has opened.
    fn block(&mut self, block: &'ast Block<'ast>) {
        self.enter(AstKind::Block(block));
        self.walk_stmts(block);
        self.leave();
    }

    /// A condition's binding, if it has one, and the node of the name it
    /// declares once the condition is read.
    fn binding_node(
        &mut self,
        binding: Option<&'ast ConditionalBinding<'ast>>,
    ) -> Option<(&'ast ConditionalBinding<'ast>, NodeId)> {
        let binding = binding?;
        self.enter(AstKind::ConditionalBinding(binding));
        let node = self.typed_name_node(&binding.name);
        self.leave();
        Some((binding, node))
    }

    fn declare_binding(&mut self, binding: Option<(&'ast ConditionalBinding<'ast>, NodeId)>) {
        if let Some((binding, node)) = binding
            && let Some(symbol) = self.declare_name(&binding.name.name, SymbolKind::Local, node)
        {
            symbol.is_read_only = binding.is_const;
        }
    }

    /// A block that opens a scope of its own, which holds the binding of the
    /// condition that guards it, if one does.
    fn scoped_block(
        &mut self,
        kind: ScopeKind,
        binding: Option<(&'ast ConditionalBinding<'ast>, NodeId)>,
        block: &'ast Block<'ast>,
    ) {
        let node = self.enter(AstKind::Block(block));
        self.push_scope(kind, node);
        self.declare_binding(binding);
        self.walk_stmts(block);
        self.pop_scope();
        self.leave();
    }

    fn mark_continue(&mut self) {
        if self.loops.len() > self.loop_base
            && let Some(LoopFrame::Repeat { first_skipped }) = self.loops.last_mut()
            && first_skipped.is_none()
        {
            *first_skipped = Some(SymbolId::from_index(self.scoping.symbols().len()));
        }
    }

    /// A function's signature, then the name of a `local function`, which
    /// its body sees, then, in a new function scope, a method's implicit
    /// `self`, its parameters, and its block.
    ///
    /// The parameters, and a `local function`'s own name, bind after the
    /// whole signature parses, so a `typeof` in it reads the enclosing scope.
    fn function(&mut self, body: &'ast FunctionBody<'ast>, binding: FunctionBinding<'ast>) {
        let body_node = self.enter(AstKind::FunctionBody(body));
        if let Some(generics) = &body.generics {
            self.visit_generic_type_list(generics);
        }
        let first_param = self.typed_name_nodes(&body.params);
        if let Some(vararg) = &body.vararg {
            self.visit_var_arg_param(vararg);
        }
        if let Some(annotation) = &body.return_type {
            self.visit_type(annotation);
        }
        let own_symbol = if let FunctionBinding::OwnName { name, declaration, kind } = binding {
            self.declare_name(name, SymbolKind::FunctionName, declaration).map(|symbol| {
                symbol.is_exported = kind.is_exported();
                symbol.is_read_only = kind.is_const();
                symbol.id
            })
        } else {
            None
        };
        self.local_functions.extend(own_symbol);

        let enclosing_loop_base = mem::replace(&mut self.loop_base, self.loops.len());
        self.push_scope(ScopeKind::Function, body_node);
        if matches!(binding, FunctionBinding::ImplicitSelf) {
            let span = Span::new(body.span.start, body.span.start);
            self.declare("self", span, SymbolKind::ImplicitSelf, body_node);
        }
        self.declare_typed_names(&body.params, first_param, SymbolKind::Parameter, false, false);
        self.block(&body.block);
        self.pop_scope();
        if own_symbol.is_some() {
            self.local_functions.pop();
        }
        self.loop_base = enclosing_loop_base;
        self.leave();
    }
}

impl<'ast> Visit<'ast> for Builder<'ast> {
    #[inline]
    fn enter_node(&mut self, kind: AstKind<'ast>) {
        self.enter(kind);
    }

    #[inline]
    fn leave_node(&mut self, _kind: AstKind<'ast>) {
        self.leave();
    }

    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        self.block(block);
    }

    fn visit_type(&mut self, ty: &'ast Type<'ast>) {
        self.type_depth += 1;
        self.walk_type(ty);
        self.type_depth -= 1;
    }

    fn visit_function_def(&mut self, function_def: &'ast FunctionDef<'ast>) {
        self.enter(AstKind::FunctionDef(function_def));
        for attribute in &function_def.attributes {
            self.visit_function_attribute(attribute);
        }
        self.function(&function_def.body, FunctionBinding::Nothing);
        self.leave();
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if let Expression::Name(name) = expr {
            self.reference_name(name, ReferenceKind::Read);
        } else {
            self.walk_expr(expr);
        }
    }

    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        self.enter(AstKind::SuffixedExpression(suffixed_expr));
        if let Expression::Name(name) = &suffixed_expr.prefix {
            if let Some(reference) = self.reference_name(name, ReferenceKind::Read)
                && let Some(calls) = &mut self.require_calls
                && let Some(call) = require_call(suffixed_expr, name)
            {
                calls.push((call, reference));
            }
        } else {
            self.visit_expr(&suffixed_expr.prefix);
        }
        for suffix in &suffixed_expr.suffixes {
            self.visit_suffix(suffix);
        }
        self.leave();
    }

    /// The values are read before a name the assignment writes is, though
    /// its target comes first, and a target's prefix and key in its place.
    fn visit_assignment(&mut self, assignment: &'ast Assignment<'ast>) {
        self.enter(AstKind::Assignment(assignment));
        let first_target = self.deferred_writes.len();
        for target in &assignment.targets {
            match target {
                Var::Name(name) => {
                    let node = self.name_node(name);
                    self.deferred_writes.push(node);
                }
                Var::Suffixed(chain) => self.visit_suffixed_expr(chain),
            }
        }
        for expr in &assignment.values {
            self.visit_expr(expr);
        }
        let names = assignment.targets.iter().filter_map(|target| match target {
            Var::Name(name) => Some(name),
            Var::Suffixed(_) => None,
        });
        for (i, name) in names.enumerate() {
            let node = self.deferred_writes[first_target + i];
            self.reference(name, ReferenceKind::Write, node);
        }
        self.deferred_writes.truncate(first_target);
        self.leave();
    }

    fn visit_compound_assignment(&mut self, compound_assignment: &'ast CompoundAssignment<'ast>) {
        self.enter(AstKind::CompoundAssignment(compound_assignment));
        match &compound_assignment.var {
            Var::Name(name) => {
                let node = self.name_node(name);
                self.visit_expr(&compound_assignment.expr);
                self.reference(name, ReferenceKind::ReadWrite, node);
            }
            Var::Suffixed(chain) => {
                self.visit_suffixed_expr(chain);
                self.visit_expr(&compound_assignment.expr);
            }
        }
        self.leave();
    }

    /// Annotations and values can't see the names being declared.
    fn visit_local_assignment(&mut self, local_assignment: &'ast LocalAssignment<'ast>) {
        self.enter(AstKind::LocalAssignment(local_assignment));
        let first_name = self.typed_name_nodes(&local_assignment.names);
        for expr in &local_assignment.exprs {
            self.visit_expr(expr);
        }
        let kind = local_assignment.kind;
        self.declare_typed_names(
            &local_assignment.names,
            first_name,
            SymbolKind::Local,
            kind.is_exported(),
            kind.is_const(),
        );
        self.leave();
    }

    fn visit_local_function(&mut self, local_function: &'ast LocalFunction<'ast>) {
        let declaration = self.enter(AstKind::LocalFunction(local_function));
        for attribute in &local_function.attributes {
            self.visit_function_attribute(attribute);
        }
        self.name_node(&local_function.name);
        let binding = FunctionBinding::OwnName {
            name: &local_function.name,
            declaration,
            kind: local_function.kind,
        };
        self.function(&local_function.body, binding);
        self.leave();
    }

    /// `function f()` writes `f`, but `function t.m()` and `function t:m()`
    /// only read `t`. Recording a write would set off unused and overwritten
    /// warnings on the usual module pattern
    /// (`local M = {} function M.f() end return M`).
    fn visit_function_decl(&mut self, function_decl: &'ast FunctionDecl<'ast>) {
        self.enter(AstKind::FunctionDecl(function_decl));
        for attribute in &function_decl.attributes {
            self.visit_function_attribute(attribute);
        }
        let name = &function_decl.name;
        self.enter(AstKind::FunctionName(name));
        let is_field_write = name.names.len() > 1 || name.method.is_some();
        for (i, part) in name.names.iter().enumerate() {
            if i == 0 {
                let kind = if is_field_write { ReferenceKind::Read } else { ReferenceKind::Write };
                self.reference_name(part, kind);
            } else {
                self.name_node(part);
            }
        }
        if let Some(method) = &name.method {
            self.name_node(method);
        }
        self.leave();
        let binding = if name.method.is_some() {
            FunctionBinding::ImplicitSelf
        } else {
            FunctionBinding::Nothing
        };
        self.function(&function_decl.body, binding);
        self.leave();
    }

    fn visit_do_block(&mut self, do_block: &'ast DoBlock<'ast>) {
        self.enter(AstKind::DoBlock(do_block));
        self.scoped_block(ScopeKind::Block, None, &do_block.block);
        self.leave();
    }

    fn visit_while_loop(&mut self, while_loop: &'ast WhileLoop<'ast>) {
        self.enter(AstKind::WhileLoop(while_loop));
        let binding = self.binding_node(while_loop.binding.as_deref());
        self.visit_expr(&while_loop.condition);
        self.loops.push(LoopFrame::WhileOrFor);
        self.scoped_block(ScopeKind::Loop, binding, &while_loop.block);
        self.loops.pop();
        self.leave();
    }

    /// The `until` condition is in the body's scope, so it sees the body's
    /// locals.
    fn visit_repeat_loop(&mut self, repeat_loop: &'ast RepeatLoop<'ast>) {
        let node = self.enter(AstKind::RepeatLoop(repeat_loop));
        self.push_scope(ScopeKind::Loop, node);
        let scope = self.current_scope;
        self.loops.push(LoopFrame::Repeat { first_skipped: None });
        self.block(&repeat_loop.block);
        let until = if let Some(LoopFrame::Repeat { first_skipped: Some(first_skipped) }) =
            self.loops.pop()
            && self.is_checking
        {
            let condition = repeat_loop.condition.span();
            Some(UntilCheck { scope, first_skipped, condition, reported: Vec::new() })
        } else {
            None
        };
        let enclosing_until = mem::replace(&mut self.until, until);
        self.visit_expr(&repeat_loop.condition);
        self.until = enclosing_until;
        self.pop_scope();
        self.leave();
    }

    fn visit_if_stmt(&mut self, if_stmt: &'ast IfStatement<'ast>) {
        self.enter(AstKind::IfStatement(if_stmt));
        let binding = self.binding_node(if_stmt.binding.as_deref());
        self.visit_expr(&if_stmt.condition);
        self.scoped_block(ScopeKind::Block, binding, &if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.enter(AstKind::ElseIfClause(clause));
            let clause_binding = self.binding_node(clause.binding.as_deref());
            self.visit_expr(&clause.condition);
            self.scoped_block(ScopeKind::Block, clause_binding, &clause.block);
            self.leave();
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.enter(AstKind::ElseClause(else_clause));
            self.scoped_block(ScopeKind::Block, None, &else_clause.block);
            self.leave();
        }
        self.leave();
    }

    /// A binding holds only its own branch's expression, in a scope that
    /// branch's node opens.
    fn visit_if_expr(&mut self, if_expr: &'ast IfExpression<'ast>) {
        let node = self.enter(AstKind::IfExpression(if_expr));
        let binding = self.binding_node(if_expr.binding.as_deref());
        self.visit_expr(&if_expr.condition);
        self.bound_expr(node, binding, &if_expr.then_expr);
        for clause in &if_expr.elseif_clauses {
            let clause_node = self.enter(AstKind::ElseIfExprClause(clause));
            let clause_binding = self.binding_node(clause.binding.as_deref());
            self.visit_expr(&clause.condition);
            self.bound_expr(clause_node, clause_binding, &clause.expr);
            self.leave();
        }
        self.visit_expr(&if_expr.else_expr);
        self.leave();
    }

    /// The loop variable binds after the header.
    fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor<'ast>) {
        let node = self.enter(AstKind::NumericFor(numeric_for));
        let variable = self.typed_name_node(&numeric_for.variable);
        self.visit_expr(&numeric_for.start);
        self.visit_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.visit_expr(step);
        }
        self.push_scope(ScopeKind::Loop, node);
        let kind = SymbolKind::NumericForVariable;
        self.declare_name(&numeric_for.variable.name, kind, variable);
        self.loops.push(LoopFrame::WhileOrFor);
        self.block(&numeric_for.block);
        self.loops.pop();
        self.pop_scope();
        self.leave();
    }

    /// Annotations and iterator expressions can't see the loop variables.
    fn visit_generic_for(&mut self, generic_for: &'ast GenericFor<'ast>) {
        let node = self.enter(AstKind::GenericFor(generic_for));
        let first_name = self.typed_name_nodes(&generic_for.names);
        for expr in &generic_for.exprs {
            self.visit_expr(expr);
        }
        self.push_scope(ScopeKind::Loop, node);
        let kind = SymbolKind::IteratorVariable;
        self.declare_typed_names(&generic_for.names, first_name, kind, false, false);
        self.loops.push(LoopFrame::WhileOrFor);
        self.block(&generic_for.block);
        self.loops.pop();
        self.pop_scope();
        self.leave();
    }

    /// The body of a type function, unlike a type alias, runs in the type
    /// runtime.
    fn visit_type_declaration(&mut self, type_declaration: &'ast TypeDeclaration<'ast>) {
        self.enter(AstKind::TypeDeclaration(type_declaration));
        self.name_node(&type_declaration.name);
        if let Some(generics) = &type_declaration.generics {
            self.visit_generic_type_list(generics);
        }
        match &type_declaration.value {
            TypeDeclarationValue::Alias(ty) => self.visit_type(ty),
            TypeDeclarationValue::TypeFunction(body) => {
                let first_scope = Some(self.scoping.next_scope_id());
                let enclosing = mem::replace(&mut self.type_function_scope, first_scope);
                self.function(body, FunctionBinding::Nothing);
                self.type_function_scope = enclosing;
            }
        }
        self.leave();
    }
}
