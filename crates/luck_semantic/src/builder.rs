mod gotos;
mod limits;

use std::mem;

use luck_ast::{
    Assignment, AstKind, Attribute, Block, Call, Chunk, CompoundAssignment, DoBlock, Expression,
    FunctionBody, FunctionDecl, FunctionDef, GenericFor, GetSpan as _, GlobalDeclaration,
    GlobalFunction, GlobalStar, GotoStatement, IfStatement, LabelStatement, LocalAssignment,
    LocalFunction, LocalFunctionKind, Name, NumericFor, RepeatLoop, Statement, SuffixedExpression,
    TreeCounts, Type, TypeDeclaration, TypeDeclarationValue, Var, Visit, WhileLoop,
};
use luck_diagnostics::Diagnostic;
use luck_token::{LuaVersion, Span};
use rustc_hash::FxHashSet;

use self::gotos::{BlockStart, FunctionStart, Gotos, Sink};
use self::limits::{Limits, Upvalue};
use crate::checker::{
    assignment_to_read_only, type_function_outer_local, undeclared_global,
    until_reads_skipped_local,
};
use crate::id::{NameId, NodeId, ReferenceId, ScopeId, SymbolId};
use crate::node::AstNodes;
use crate::require_site::{require_call, require_sites};
use crate::scoping::{
    Label, NameRole, Reference, ReferenceKind, Scope, ScopeKind, Scoping, Symbol, SymbolKind,
};
use crate::semantic::Semantic;

/// One walk over a chunk, in source order, that builds its [`Semantic`].
///
/// It always resolves every name with the chunk's version's scoping rules
/// into a [`Scoping`]. Each other part is built only when asked for: the
/// checker's errors, the node table, and the `require` sites.
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

    /// Reports the errors Lua's compiler and Luau's parser make beyond the
    /// grammar: a write to a read-only binding, a free name no Lua 5.5
    /// `global` declaration covers, a `goto` no label takes or one that jumps
    /// into the scope of a local, a duplicate label, a Luau type function that
    /// reads a local from outside it, a Luau `until` that reads a local a
    /// `continue` skips, and in Lua 5.1 to 5.5, a function with more active
    /// locals or upvalues than its compiler allows.
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

    pub fn build<'ast>(self, chunk: &'ast Chunk<'ast>) -> SemanticBuilderReturn<'ast> {
        let counts = self.counts;
        let names = counts.names as usize;
        let distinct_names = counts.declarations as usize + counts.name_uses as usize;
        let module = ScopeId::from_index(0);
        let mut builder = Builder {
            version: chunk.version,
            is_checking: self.is_checking,
            limits: if self.is_checking {
                Limits::new(chunk.version, counts.declarations as usize)
            } else {
                None
            },
            scoping: Scoping::with_capacity(chunk.version, counts),
            // A chunk has about four nodes for each name.
            nodes: self.is_building_nodes.then(|| AstNodes::with_capacity(names * 4)),
            current_node: None,
            next_node: NodeId::from_index(0),
            require_calls: self.is_finding_requires.then(Vec::new),
            errors: Vec::new(),
            bindings: Vec::with_capacity(distinct_names),
            current_scope: module,
            current_function: module,
            outer_scopes: Vec::new(),
            shadowed: Vec::new(),
            env_name: None,
            global_star: None,
            named_globals: 0,
            active_vars: 0,
            gotos: Gotos::default(),
            loops: Vec::new(),
            loop_base: 0,
            until: None,
            type_function_scope: None,
            type_depth: 0,
            local_functions: Vec::new(),
            type_names: FxHashSet::default(),
            scratch_nodes: Vec::new(),
            deferred_writes: Vec::new(),
        };
        builder.visit_chunk(chunk);
        builder.finish()
    }
}

/// A name's innermost live declaration.
#[derive(Debug, Clone, Copy)]
enum Binding {
    Free,
    Local(SymbolId),
    /// Lua 5.5: a `global` declaration, which voids any enclosing local of
    /// the name for the rest of its block.
    Global,
    /// Lua 5.5: a `global <const>` declaration.
    ReadOnlyGlobal,
}

impl Binding {
    const fn local(self) -> Option<SymbolId> {
        match self {
            Self::Local(id) => Some(id),
            Self::Free | Self::Global | Self::ReadOnlyGlobal => None,
        }
    }
}

/// What a scope's exit restores.
struct OuterScope {
    scope: ScopeId,
    function: ScopeId,
    shadowed_len: usize,
    global_star: Option<bool>,
    named_globals: u32,
    /// Also the count where a label that ends the inner block resolves.
    active_vars: usize,
    gotos: BlockStart,
}

/// Luau: a loop a `continue` goes on with.
enum LoopFrame {
    WhileOrFor,
    Repeat {
        /// The first symbol its body declares after its first `continue`.
        first_skipped: Option<SymbolId>,
    },
}

/// Luau: an `until` condition whose loop body declares locals after a
/// `continue`, which the condition may not read.
struct UntilCheck {
    scope: ScopeId,
    first_skipped: SymbolId,
    condition: Span,
    reported: Vec<SymbolId>,
}

/// What a function binds besides its parameters and its vararg.
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
    version: LuaVersion,
    is_checking: bool,
    /// Lua 5.1 to 5.5: what counts toward each function's limits, while
    /// checking.
    limits: Option<Limits>,
    scoping: Scoping,
    nodes: Option<AstNodes<'ast>>,
    /// The innermost node the walk is in, while it builds the table. Leaving
    /// a node moves this to the parent the table holds for it.
    current_node: Option<NodeId>,
    next_node: NodeId,
    /// Each [`require_call`], with the reference its prefix makes.
    require_calls: Option<Vec<(Call<'ast>, ReferenceId)>>,
    errors: Vec<Diagnostic>,
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
    /// Lua 5.5: whether a `global *` is live, and if so whether it's
    /// read-only.
    global_star: Option<bool>,
    /// Lua 5.5: how many `global` declarations of a name are live, which
    /// leave every other free name undeclared.
    named_globals: u32,
    /// The locals and global declarations in open scopes, which a `goto`
    /// can't jump past.
    active_vars: usize,
    gotos: Gotos,
    /// Luau: the open loops, of every function, and where the current
    /// function's start.
    loops: Vec<LoopFrame>,
    loop_base: usize,
    until: Option<UntilCheck>,
    /// Luau: the first scope of the innermost type function, so every scope
    /// before it is outside it.
    type_function_scope: Option<ScopeId>,
    /// Luau: how many types the walk is in. Types never run, so a reference
    /// in one captures nothing.
    type_depth: u32,
    /// The `local function`s whose bodies the walk is in, innermost last.
    local_functions: Vec<SymbolId>,
    /// Luau: every type declared by a block entered so far.
    type_names: FxHashSet<&'ast str>,
    /// The nodes of names a statement binds once its values are read,
    /// innermost statement's last.
    scratch_nodes: Vec<NodeId>,
    /// The node of each name an assignment writes once its values are read,
    /// and its slot among the references by position, innermost
    /// assignment's last.
    deferred_writes: Vec<(NodeId, usize)>,
}

fn is_read_only_attribute(attribute: &Attribute<'_>) -> bool {
    matches!(attribute.name.text.as_str(), "const" | "close")
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

    fn intern(&mut self, text: &str) -> NameId {
        let id = self.scoping.intern(text);
        if self.bindings.len() <= id.index() {
            self.bindings.push(Binding::Free);
        }
        id
    }

    fn binding(&self, name: NameId) -> Binding {
        self.bindings[name.index()]
    }

    fn bind(&mut self, name: NameId, binding: Binding) {
        let previous = mem::replace(&mut self.bindings[name.index()], binding);
        self.shadowed.push((name, previous));
    }

    fn push_scope(&mut self, kind: ScopeKind, node: NodeId) {
        let id = self.scoping.next_scope_id();
        let function = if kind == ScopeKind::Function { id } else { self.current_function };
        self.scoping.add_scope(Scope {
            parent: Some(self.current_scope),
            kind,
            function,
            node,
            has_vararg_read: false,
        });
        let gotos = if self.version.has_goto() {
            self.gotos.open_block(self.active_vars)
        } else {
            BlockStart::default()
        };
        self.outer_scopes.push(OuterScope {
            scope: mem::replace(&mut self.current_scope, id),
            function: mem::replace(&mut self.current_function, function),
            shadowed_len: self.shadowed.len(),
            global_star: self.global_star,
            named_globals: self.named_globals,
            active_vars: self.active_vars,
            gotos,
        });
        if let Some(limits) = &mut self.limits {
            limits.enter_scope();
        }
    }

    fn pop_scope(&mut self) {
        let outer = self.outer_scopes.pop().expect("a scope's exit follows its entry");
        if let Some(limits) = &mut self.limits {
            limits.leave_scope();
        }
        self.current_scope = outer.scope;
        self.current_function = outer.function;
        self.global_star = outer.global_star;
        self.named_globals = outer.named_globals;
        self.active_vars = outer.active_vars;
        if self.version.has_goto() {
            let sink = &mut Sink::new(&mut self.scoping, &mut self.errors, self.is_checking);
            self.gotos.close_block(outer.gotos, sink);
        }
        // Innermost first, since a scope can declare one name twice.
        for (name, previous) in self.shadowed.drain(outer.shadowed_len..).rev() {
            self.bindings[name.index()] = previous;
        }
    }

    /// The active variables where the innermost open block began, which is
    /// where a label that ends it resolves.
    fn block_start_vars(&self) -> usize {
        self.outer_scopes.last().map_or(0, |outer| outer.active_vars)
    }

    /// Declares a local, which is a Lua 5.4+ compile-time constant when
    /// `is_constant`.
    fn declare(
        &mut self,
        text: &str,
        span: Span,
        kind: SymbolKind,
        declaration: NodeId,
        is_constant: bool,
    ) -> SymbolId {
        let name = self.intern(text);
        let shadows = self.binding(name).local();
        let id =
            self.scoping.add_symbol(name, self.current_scope, kind, declaration, span, shadows);
        self.bind(name, Binding::Local(id));
        self.active_vars += 1;
        if self.version.has_env_upvalue() && text == "_ENV" {
            self.env_name = Some(name);
        }
        if let Some(limits) = &mut self.limits {
            limits.local(id, span, is_constant, &mut self.errors);
        }
        id
    }

    /// `None` for a name the parser found missing.
    fn declare_name(
        &mut self,
        name: &Name<'_>,
        kind: SymbolKind,
        declaration: NodeId,
        is_constant: bool,
    ) -> Option<&mut Symbol> {
        if name.text.is_empty() {
            return None;
        }
        let id = self.declare(name.text.as_str(), name.span, kind, declaration, is_constant);
        self.scoping.set_role(name.index, NameRole::Symbol(id));
        Some(self.scoping.symbol_mut(id))
    }

    /// `count` locals no name spells, which the construct at `span` declares,
    /// as a `for` loop's state.
    fn hidden_locals(&mut self, count: usize, span: Span) {
        if let Some(limits) = &mut self.limits {
            limits.hidden(count, span, &mut self.errors);
        }
    }

    /// Lua 5.5: a `global` declaration of `name`, which a value assigns when
    /// `is_assigned`.
    fn declare_global(&mut self, name: &Name<'_>, is_assigned: bool, is_read_only: bool) {
        if name.text.is_empty() {
            return;
        }
        let id = self.intern(name.text.as_str());
        self.scoping.add_declared_global(id, self.current_scope);
        if is_assigned {
            self.scoping.mark_assigned_global(id);
        }
        self.bind(id, if is_read_only { Binding::ReadOnlyGlobal } else { Binding::Global });
        self.named_globals += 1;
        self.active_vars += 1;
    }

    /// Records a use of `name`, whose node is `node`, resolved against the
    /// live declarations, in `position` among the references by position when
    /// it reserved one there. A use from a nested function also marks the
    /// symbol it reads, or the local `_ENV` it indexes, as an upvalue, and a
    /// write marks a symbol modified.
    fn reference(
        &mut self,
        name: &Name<'_>,
        kind: ReferenceKind,
        node: NodeId,
        position: Option<usize>,
    ) -> Option<ReferenceId> {
        let text = name.text.as_str();
        if text.is_empty() {
            return None;
        }
        let name_id = self.intern(text);
        let binding = self.binding(name_id);
        let resolved = binding.local();
        // Luau: `types` and the types in scope belong to the type runtime, so
        // inside a type function they're no references of this chunk.
        if resolved.is_none()
            && self.type_function_scope.is_some()
            && (text == "types" || self.type_names.contains(text))
        {
            return None;
        }
        if self.is_checking {
            self.check_reference(name, kind, binding);
        }
        let (env, is_declared_global) = match resolved {
            Some(_) => (None, false),
            None => (
                self.env_name.and_then(|env| self.binding(env).local()),
                matches!(binding, Binding::Global | Binding::ReadOnlyGlobal)
                    || self.global_star.is_some(),
            ),
        };
        if kind.is_write() && resolved.is_none() && env.is_none() {
            self.scoping.mark_assigned_global(name_id);
        }
        let captured = if let Some(symbol_id) = resolved.or(env) {
            let is_captured = self.type_depth == 0
                && self.scoping.scope(self.scoping.symbol(symbol_id).scope).function
                    != self.current_function;
            let symbol = self.scoping.symbol_mut(symbol_id);
            symbol.is_upvalue |= is_captured;
            symbol.is_modified |= resolved.is_some() && kind.is_write();
            is_captured.then_some(Upvalue::Symbol(symbol_id))
        } else {
            (self.version.has_env_upvalue() && self.type_depth == 0).then_some(Upvalue::Env)
        };
        if let Some(upvalue) = captured
            && let Some(limits) = &mut self.limits
        {
            limits.capture(upvalue, name.span, &mut self.errors);
        }
        let reference = Reference {
            span: name.span,
            name: name_id,
            scope: self.current_scope,
            node,
            resolved,
            env,
            kind,
            is_declared_global,
            is_in_type: self.type_depth > 0,
            is_recursive: resolved.is_some_and(|symbol| self.local_functions.contains(&symbol)),
        };
        let id = self.scoping.add_reference(reference, position);
        self.scoping.set_role(name.index, NameRole::Reference(id));
        Some(id)
    }

    /// A name's node and the reference it makes.
    fn reference_name(
        &mut self,
        name: &'ast Name<'ast>,
        kind: ReferenceKind,
    ) -> Option<ReferenceId> {
        let node = self.enter(AstKind::Name(name));
        let reference = self.reference(name, kind, node, None);
        self.leave();
        reference
    }

    /// The checker's errors for a use of `name`, which `binding` declares.
    fn check_reference(&mut self, name: &Name<'_>, kind: ReferenceKind, binding: Binding) {
        let text = name.text.as_str();
        match binding {
            Binding::Local(id) => {
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
            // Lua 5.5
            Binding::Global => {}
            // Lua 5.5
            Binding::ReadOnlyGlobal => {
                if kind.is_write() {
                    self.errors.push(assignment_to_read_only(name.span, text));
                }
            }
            // `global *` covers a name no declaration of its own does, and
            // otherwise a Lua 5.5 `global` leaves it undeclared. The chunk's
            // `_ENV` is always declared.
            Binding::Free => {
                if !self.version.has_global() || text == "_ENV" {
                    return;
                }
                match self.global_star {
                    Some(is_read_only) => {
                        if kind.is_write() && is_read_only {
                            self.errors.push(assignment_to_read_only(name.span, text));
                        }
                    }
                    None => {
                        if self.named_globals > 0 {
                            self.errors.push(undeclared_global(name.span, text));
                        }
                    }
                }
            }
        }
    }

    /// Luau: a type function sees every type of its block, earlier or later.
    fn record_type_names(&mut self, block: &'ast Block<'ast>) {
        for stmt in &block.stmts {
            if let Statement::TypeDeclaration(declaration) = stmt {
                self.type_names.insert(declaration.name.text.as_str());
            }
        }
    }

    /// The statements of a block whose scope is open, the body of a `repeat`
    /// when `is_repeat_body`.
    fn walk_stmts(&mut self, block: &'ast Block<'ast>, is_repeat_body: bool) {
        if self.version.has_type_syntax() {
            self.record_type_names(block);
        }
        // `block_follow(ls, 0)`: a label ends its block only if nothing but
        // labels follow, not `return`, and not a repeat's `until`.
        let trailing_start = if is_repeat_body || !self.version.has_goto() {
            block.stmts.len()
        } else {
            block
                .stmts
                .iter()
                .rposition(|stmt| !matches!(stmt, Statement::Label(_)))
                .map_or(0, |i| i + 1)
        };
        for (i, stmt) in block.stmts.iter().enumerate() {
            if let Statement::Label(label) = stmt {
                let active_vars =
                    if i >= trailing_start { self.block_start_vars() } else { self.active_vars };
                self.label(label, active_vars);
            } else {
                if matches!(stmt, Statement::Continue(_)) {
                    self.mark_continue();
                }
                self.visit_stmt(stmt);
            }
        }
    }

    /// A block whose scope its parent has opened.
    fn block(&mut self, block: &'ast Block<'ast>) {
        self.enter(AstKind::Block(block));
        self.walk_stmts(block, false);
        self.leave();
    }

    /// A block that opens a scope of its own.
    fn scoped_block(&mut self, kind: ScopeKind, block: &'ast Block<'ast>) {
        let node = self.enter(AstKind::Block(block));
        self.push_scope(kind, node);
        self.walk_stmts(block, false);
        self.pop_scope();
        self.leave();
    }

    // Lua 5.2+
    fn label(&mut self, label: &'ast LabelStatement<'ast>, active_vars: usize) {
        let node = self.enter(AstKind::LabelStatement(label));
        self.name_node(&label.name);
        self.leave();
        if label.name.text.is_empty() {
            return;
        }
        let name = self.intern(label.name.text.as_str());
        let id = self.scoping.add_label(Label {
            name,
            span: label.span,
            scope: self.current_scope,
            node,
            is_used: false,
        });
        self.scoping.set_role(label.name.index, NameRole::Label(id));
        let sink = &mut Sink::new(&mut self.scoping, &mut self.errors, self.is_checking);
        self.gotos.add_label(name, id, active_vars, self.version, sink);
    }

    // Luau
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
    /// `self`, its parameters, its vararg's name or Lua 5.1's implicit `arg`,
    /// and its block.
    ///
    /// Luau binds the parameters, and a `local function`'s own name, after
    /// the whole signature parses, so a `typeof` in it reads the enclosing
    /// scope.
    ///
    /// `start` is where what spells the function starts. From there to its
    /// parameters is its header, which the checker's errors about the whole
    /// function point at.
    fn function(
        &mut self,
        body: &'ast FunctionBody<'ast>,
        binding: FunctionBinding<'ast>,
        start: u32,
    ) {
        let body_node = self.enter(AstKind::FunctionBody(body));
        if let Some(generics) = &body.generics {
            self.visit_generic_type_list(generics);
        }
        let first_param = self.scratch_nodes.len();
        for param in &body.params {
            let node = self.enter(AstKind::Parameter(param));
            self.name_node(&param.name);
            if let Some(annotation) = &param.type_annotation {
                self.visit_type(annotation);
            }
            self.leave();
            self.scratch_nodes.push(node);
        }
        let vararg_node = body.vararg.as_deref().map(|vararg| {
            let node = self.enter(AstKind::VarArgParam(vararg));
            if let Some(name) = &vararg.name {
                self.name_node(name);
            }
            if let Some(annotation) = &vararg.type_annotation {
                self.visit_type(annotation);
            }
            self.leave();
            node
        });
        if let Some(annotation) = &body.return_type {
            self.visit_type(annotation);
        }
        let own_symbol = if let FunctionBinding::OwnName { name, declaration, kind } = binding {
            self.declare_name(name, SymbolKind::FunctionName, declaration, false).map(|symbol| {
                symbol.is_exported = kind.is_exported(); // Luau
                symbol.is_read_only = kind.is_const(); // Luau
                symbol.id
            })
        } else {
            None
        };
        self.local_functions.extend(own_symbol);

        let enclosing_gotos = if self.version.has_goto() {
            self.gotos.enter_function()
        } else {
            FunctionStart::default()
        };
        let enclosing_loop_base = mem::replace(&mut self.loop_base, self.loops.len());
        if let Some(limits) = &mut self.limits {
            limits.enter_function(Span::new(start, body.span.start));
        }
        self.push_scope(ScopeKind::Function, body_node);
        if matches!(binding, FunctionBinding::ImplicitSelf) {
            let span = Span::new(body.span.start, body.span.start);
            self.declare("self", span, SymbolKind::ImplicitSelf, body_node, false);
        }
        for (i, param) in body.params.iter().enumerate() {
            let node = self.scratch_nodes[first_param + i];
            self.declare_name(&param.name, SymbolKind::Parameter, node, false);
        }
        self.scratch_nodes.truncate(first_param);
        if let (Some(vararg), Some(node)) = (body.vararg.as_deref(), vararg_node) {
            // Lua 5.5: `...name` binds the vararg table to a read-only local,
            // and a plain `...` to a hidden one.
            if let Some(name) = &vararg.name
                && let Some(symbol) = self.declare_name(name, SymbolKind::Parameter, node, false)
            {
                symbol.is_read_only = true;
            }
            if vararg.name.is_none() && self.version.has_hidden_vararg_table() {
                self.hidden_locals(1, vararg.span);
            }
            // Lua 5.1: a vararg function binds its extra arguments' table to
            // `arg`, after its parameters, as `parlist` declares it.
            if self.version.has_implicit_vararg_table() {
                let span = Span::new(vararg.span.start, vararg.span.start);
                self.declare("arg", span, SymbolKind::ImplicitArg, node, false);
            }
        }
        self.block(&body.block);
        self.pop_scope();
        if let Some(limits) = &mut self.limits {
            limits.leave_function();
        }
        if own_symbol.is_some() {
            self.local_functions.pop();
        }
        self.loop_base = enclosing_loop_base;
        if self.version.has_goto() {
            let sink = &mut Sink::new(&mut self.scoping, &mut self.errors, self.is_checking);
            self.gotos.leave_function(enclosing_gotos, sink);
        }
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

    fn visit_chunk(&mut self, chunk: &'ast Chunk<'ast>) {
        let node = self.enter(AstKind::Chunk(chunk));
        let module = self.current_scope;
        self.scoping.add_scope(Scope {
            parent: None,
            kind: ScopeKind::Module,
            function: module,
            node,
            has_vararg_read: false,
        });
        let start =
            if self.version.has_goto() { self.gotos.open_block(0) } else { BlockStart::default() };
        self.block(&chunk.block);
        if self.version.has_goto() {
            let sink = &mut Sink::new(&mut self.scoping, &mut self.errors, self.is_checking);
            self.gotos.close_block(start, sink);
            self.gotos.leave_function(FunctionStart::default(), sink);
        }
        self.leave();
    }

    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        self.block(block);
    }

    // Luau
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
        self.function(&function_def.body, FunctionBinding::Nothing, function_def.span.start);
        self.leave();
    }

    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if let Expression::Name(name) = expr {
            self.reference_name(name, ReferenceKind::Read);
        } else if let Expression::VarArg(_) = expr {
            self.scoping.mark_vararg_read(self.current_function);
        } else {
            self.walk_expr(expr);
        }
    }

    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        self.enter(AstKind::SuffixedExpression(suffixed_expr));
        if let Expression::Name(name) = &suffixed_expr.prefix {
            if let Some(reference) = self.reference_name(name, ReferenceKind::Read)
                && let Some(calls) = &mut self.require_calls
                && let Some(call) = require_call(suffixed_expr, name, self.version)
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
                    let position = self.scoping.reserve_position();
                    self.deferred_writes.push((node, position));
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
            let (node, position) = self.deferred_writes[first_target + i];
            self.reference(name, ReferenceKind::Write, node, Some(position));
        }
        self.deferred_writes.truncate(first_target);
        self.leave();
    }

    // Luau
    fn visit_compound_assignment(&mut self, compound_assignment: &'ast CompoundAssignment<'ast>) {
        self.enter(AstKind::CompoundAssignment(compound_assignment));
        match &compound_assignment.var {
            Var::Name(name) => {
                let node = self.name_node(name);
                let position = self.scoping.reserve_position();
                self.visit_expr(&compound_assignment.expr);
                self.reference(name, ReferenceKind::ReadWrite, node, Some(position));
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
        let leading = local_assignment.attribute.as_deref();
        if let Some(attribute) = leading {
            self.visit_attribute(attribute);
        }
        let first_name = self.scratch_nodes.len();
        for attributed in &local_assignment.names {
            let node = self.enter(AstKind::AttributedName(attributed));
            self.name_node(&attributed.name);
            if let Some(annotation) = &attributed.type_annotation {
                self.visit_type(annotation);
            }
            if let Some(attribute) = &attributed.attribute {
                self.visit_attribute(attribute);
            }
            self.leave();
            self.scratch_nodes.push(node);
        }
        for expr in &local_assignment.exprs {
            self.visit_expr(expr);
        }
        let kind = local_assignment.kind;
        let last = local_assignment.names.len().saturating_sub(1);
        // Lua 5.4+: only the last of as many names as values can be one.
        let is_last_constant = self.version.has_compile_time_constants()
            && local_assignment.names.len() == local_assignment.exprs.len()
            && local_assignment.names.last().is_some_and(|attributed| {
                attributed.attribute_or(leading).is_some_and(|given| given.name.text == "const")
            })
            && local_assignment.exprs.last().is_some_and(|expr| {
                self.limits.as_ref().is_some_and(|limits| limits.is_constant(expr, &self.scoping))
            });
        for (i, attributed) in local_assignment.names.iter().enumerate() {
            let node = self.scratch_nodes[first_name + i];
            let is_constant = is_last_constant && i == last;
            let Some(symbol) =
                self.declare_name(&attributed.name, SymbolKind::Local, node, is_constant)
            else {
                continue;
            };
            let attribute = attributed.attribute_or(leading);
            symbol.is_exported = kind.is_exported(); // Luau
            symbol.is_close = attribute.is_some_and(|given| given.name.text == "close");
            symbol.is_read_only = kind.is_const() || attribute.is_some_and(is_read_only_attribute);
        }
        self.scratch_nodes.truncate(first_name);
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
        self.function(&local_function.body, binding, local_function.span.start);
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
        self.function(&function_decl.body, binding, function_decl.span.start);
        self.leave();
    }

    fn visit_do_block(&mut self, do_block: &'ast DoBlock<'ast>) {
        self.enter(AstKind::DoBlock(do_block));
        self.scoped_block(ScopeKind::Block, &do_block.block);
        self.leave();
    }

    fn visit_while_loop(&mut self, while_loop: &'ast WhileLoop<'ast>) {
        self.enter(AstKind::WhileLoop(while_loop));
        self.visit_expr(&while_loop.condition);
        self.loops.push(LoopFrame::WhileOrFor);
        self.scoped_block(ScopeKind::Loop, &while_loop.block);
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
        self.enter(AstKind::Block(&repeat_loop.block));
        self.walk_stmts(&repeat_loop.block, true);
        self.leave();
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
        self.visit_expr(&if_stmt.condition);
        self.scoped_block(ScopeKind::Block, &if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.enter(AstKind::ElseIfClause(clause));
            self.visit_expr(&clause.condition);
            self.scoped_block(ScopeKind::Block, &clause.block);
            self.leave();
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.enter(AstKind::ElseClause(else_clause));
            self.scoped_block(ScopeKind::Block, &else_clause.block);
            self.leave();
        }
        self.leave();
    }

    /// The loop variable binds after the header.
    fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor<'ast>) {
        let node = self.enter(AstKind::NumericFor(numeric_for));
        self.name_node(&numeric_for.name);
        if let Some(annotation) = &numeric_for.type_annotation {
            self.visit_type(annotation);
        }
        self.visit_expr(&numeric_for.start);
        self.visit_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.visit_expr(step);
        }
        self.push_scope(ScopeKind::Loop, node);
        self.hidden_locals(self.version.numeric_for_state_locals(), numeric_for.name.span);
        let is_read_only = self.version.has_const_for_variables();
        if let Some(symbol) =
            self.declare_name(&numeric_for.name, SymbolKind::NumericForVariable, node, false)
        {
            symbol.is_read_only = is_read_only; // Lua 5.5
        }
        self.loops.push(LoopFrame::WhileOrFor);
        self.block(&numeric_for.block);
        self.loops.pop();
        self.pop_scope();
        self.leave();
    }

    /// Annotations and iterator expressions can't see the loop variables.
    fn visit_generic_for(&mut self, generic_for: &'ast GenericFor<'ast>) {
        let node = self.enter(AstKind::GenericFor(generic_for));
        let first_name = self.scratch_nodes.len();
        for binding in &generic_for.names {
            let param = self.enter(AstKind::Parameter(binding));
            self.name_node(&binding.name);
            if let Some(annotation) = &binding.type_annotation {
                self.visit_type(annotation);
            }
            self.leave();
            self.scratch_nodes.push(param);
        }
        for expr in &generic_for.exprs {
            self.visit_expr(expr);
        }
        self.push_scope(ScopeKind::Loop, node);
        let first_name_span =
            generic_for.names.first().map_or(generic_for.span, |first| first.name.span);
        self.hidden_locals(self.version.generic_for_state_locals(), first_name_span);
        let has_const_control = self.version.has_const_for_variables();
        for (i, binding) in generic_for.names.iter().enumerate() {
            let declaration = self.scratch_nodes[first_name + i];
            if let Some(symbol) =
                self.declare_name(&binding.name, SymbolKind::IteratorVariable, declaration, false)
            {
                // Lua 5.5: only the first variable is the control variable.
                symbol.is_read_only = i == 0 && has_const_control;
            }
        }
        self.scratch_nodes.truncate(first_name);
        self.loops.push(LoopFrame::WhileOrFor);
        self.block(&generic_for.block);
        self.loops.pop();
        self.pop_scope();
        self.leave();
    }

    // Lua 5.2+
    fn visit_goto_stmt(&mut self, goto_stmt: &'ast GotoStatement<'ast>) {
        self.walk_goto_stmt(goto_stmt);
        if goto_stmt.name.text.is_empty() {
            return;
        }
        let name = self.intern(goto_stmt.name.text.as_str());
        let sink = &mut Sink::new(&mut self.scoping, &mut self.errors, self.is_checking);
        self.gotos.add_goto(name, goto_stmt.name.index, goto_stmt.span, self.active_vars, sink);
    }

    // Lua 5.5: the initializers can't see the names being declared, and a
    // name with an initializer is assigned.
    fn visit_global_declaration(&mut self, global_declaration: &'ast GlobalDeclaration<'ast>) {
        self.enter(AstKind::GlobalDeclaration(global_declaration));
        let leading = global_declaration.attribute.as_deref();
        if let Some(attribute) = leading {
            self.visit_attribute(attribute);
        }
        for attributed in &global_declaration.names {
            self.visit_attributed_name(attributed);
        }
        for expr in &global_declaration.exprs {
            self.visit_expr(expr);
        }
        let is_assigned = !global_declaration.exprs.is_empty();
        for attributed in &global_declaration.names {
            let is_read_only = attributed.attribute_or(leading).is_some_and(is_read_only_attribute);
            self.declare_global(&attributed.name, is_assigned, is_read_only);
        }
        self.leave();
    }

    // Lua 5.5: declared before the body, so the body reads the global.
    fn visit_global_function(&mut self, global_function: &'ast GlobalFunction<'ast>) {
        self.enter(AstKind::GlobalFunction(global_function));
        self.name_node(&global_function.name);
        self.declare_global(&global_function.name, true, false);
        self.function(&global_function.body, FunctionBinding::Nothing, global_function.span.start);
        self.leave();
    }

    // Lua 5.5
    fn visit_global_star(&mut self, global_star: &'ast GlobalStar<'ast>) {
        self.walk_global_star(global_star);
        self.global_star = Some(global_star.attribute.as_ref().is_some_and(is_read_only_attribute));
        self.active_vars += 1;
    }

    // Luau: the body of a type function, unlike a type alias, runs in the
    // type runtime.
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
                self.function(body, FunctionBinding::Nothing, type_declaration.span.start);
                self.type_function_scope = enclosing;
            }
        }
        self.leave();
    }
}
