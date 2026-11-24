mod gotos;

use core::mem;

use luck_ast::{
    Assignment, AstKind, Attribute, Block, Call, Chunk, CompoundAssignment, DoBlock, Expression,
    FunctionArgs, FunctionBody, FunctionDecl, GenericFor, GetSpan as _, GlobalDeclaration,
    GlobalFunction, GlobalStar, GotoStatement, IfStatement, LabelStatement, LocalAssignment,
    LocalFunction, LocalFunctionKind, Name, NumericFor, RepeatLoop, Statement, StringLiteral,
    Suffix, SuffixedExpression, TreeCounts, Type, TypeDeclaration, TypeDeclarationValue, Var,
    Visit, WhileLoop,
};
use luck_diagnostics::Diagnostic;
use luck_token::{LuaVersion, Span, offset_to_index};
use rustc_hash::FxHashSet;

use self::gotos::{BlockStart, FunctionGotos, Gotos, Sink};
use crate::checker::{
    assignment_to_read_only, type_function_outer_local, undeclared_global,
    until_reads_skipped_local,
};
use crate::id::{NameId, NodeId, ReferenceId, ScopeId, SymbolId};
use crate::node::AstNodes;
use crate::scoping::{
    Label, NameRole, Reference, ReferenceKind, Scope, ScopeKind, Scoping, Symbol, SymbolKind,
};
use crate::semantic::{RequireSite, Semantic};

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
    pub diagnostics: Vec<Diagnostic>,
}

impl SemanticBuilder {
    /// A build of the scoping alone.
    #[must_use]
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
    /// reads a local from outside it, and a Luau `until` that reads a local a
    /// `continue` skips.
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

    /// Finds the [`RequireSite`]s.
    #[must_use]
    pub const fn with_requires(mut self, is_finding_requires: bool) -> Self {
        self.is_finding_requires = is_finding_requires;
        self
    }

    #[must_use]
    pub fn build<'ast>(self, chunk: &'ast Chunk<'ast>) -> SemanticBuilderReturn<'ast> {
        let counts = self.counts;
        let names = offset_to_index(counts.names);
        let distinct_names =
            offset_to_index(counts.declarations).saturating_add(offset_to_index(counts.name_uses));
        let module = ScopeId::from_index(0);
        let mut builder = Builder {
            version: chunk.version,
            is_checking: self.is_checking,
            scoping: Scoping::with_capacity(chunk.version, counts),
            // A chunk has about four nodes for each name.
            nodes: self.is_building_nodes.then(|| AstNodes::with_capacity(names.saturating_mul(4))),
            node_stack: Vec::new(),
            next_node: NodeId::from_index(0),
            require_calls: self.is_finding_requires.then(Vec::new),
            diagnostics: Vec::new(),
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
            type_function_floor: None,
            type_depth: 0,
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

/// Luau: the loop a `continue` goes on with.
struct LoopFrame {
    is_repeat: bool,
    /// For a `repeat` body, the first symbol declared after its first
    /// `continue`.
    first_skipped: Option<usize>,
}

/// Luau: an `until` condition whose loop body declares locals after a
/// `continue`, which the condition may not read.
struct UntilCheck {
    scope: ScopeId,
    first_skipped: usize,
    condition: Span,
    reported: Vec<SymbolId>,
}

/// The name and node of a `local function`, which binds after its signature
/// and before its body.
#[derive(Clone, Copy)]
struct OwnName<'ast> {
    name: &'ast Name<'ast>,
    declaration: NodeId,
    kind: LocalFunctionKind,
}

struct Builder<'ast> {
    version: LuaVersion,
    is_checking: bool,
    scoping: Scoping,
    nodes: Option<AstNodes<'ast>>,
    /// The nodes the walk is in, innermost last, while it builds the table.
    node_stack: Vec<NodeId>,
    next_node: NodeId,
    /// Each call of a name `require`, with the reference its callee makes.
    require_calls: Option<Vec<(Call<'ast>, ReferenceId)>>,
    diagnostics: Vec<Diagnostic>,
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
    type_function_floor: Option<usize>,
    /// Luau: how many types the walk is in. Types never run, so a reference
    /// in one captures nothing.
    type_depth: u32,
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

/// The non-method call `chain` opens with when it calls a name `require`, or
/// reads `require` from the global table `prefix` names and calls it, as
/// `_G.require(path)` or Lua 5.2+'s `_ENV.require(path)`.
fn require_call<'ast>(
    chain: &'ast SuffixedExpression<'ast>,
    prefix: &Name<'_>,
    version: LuaVersion,
) -> Option<Call<'ast>> {
    let callee_suffixes = if prefix.text == "require" {
        0
    } else if (prefix.text == "_G" || (prefix.text == "_ENV" && version.has_env_upvalue()))
        && is_key(chain.suffixes.first(), "require")
    {
        1
    } else {
        return None;
    };
    let call = Call::new(chain.head(callee_suffixes), chain.suffixes.get(callee_suffixes)?)?;
    call.method.is_none().then_some(call)
}

/// Whether `suffix` reads the field `key`, as `.key` or `["key"]`, however
/// the string is spelled.
fn is_key(suffix: &Suffix<'_>, key: &str) -> bool {
    match suffix {
        Suffix::Field(name) => name.text == key,
        Suffix::Index { index: Expression::StringLiteral(literal), .. } => {
            literal.value == key.as_bytes()
        }
        Suffix::Index { .. }
        | Suffix::Call(_)
        | Suffix::MethodCall(_)
        | Suffix::TypeInstantiation { .. } => false,
    }
}

/// The module path of a `require` call that passes one string literal and
/// nothing else.
fn require_path<'ast>(
    function_args: &'ast FunctionArgs<'ast>,
) -> Option<&'ast StringLiteral<'ast>> {
    match function_args {
        FunctionArgs::Parenthesized { args, .. } => {
            let [Expression::StringLiteral(literal)] = &**args else {
                return None;
            };
            Some(literal)
        }
        FunctionArgs::StringLiteral(literal) => Some(literal),
        FunctionArgs::TableConstructor(_) | FunctionArgs::Error(_) => None,
    }
}

impl<'ast> Builder<'ast> {
    fn finish(mut self) -> SemanticBuilderReturn<'ast> {
        self.scoping.finish();
        let scoping = &self.scoping;
        // `_G.require` is the global `require` only while the chunk leaves
        // that global alone too.
        let is_require_assigned =
            scoping.name_id("require").is_some_and(|require| scoping.is_assigned_global(require));
        let requires = self
            .require_calls
            .unwrap_or_default()
            .into_iter()
            .filter(|&(call, reference)| {
                scoping.is_host_global(scoping.reference(reference))
                    && (call.callee.suffixes.is_empty() || !is_require_assigned)
            })
            .map(|(call, _)| RequireSite { call, path: require_path(call.args) })
            .collect();
        self.diagnostics.sort_by_key(|diag| diag.span.map(|span| span.start));
        SemanticBuilderReturn {
            semantic: Semantic::new(self.scoping, self.nodes.unwrap_or_default(), requires),
            diagnostics: self.diagnostics,
        }
    }

    /// Opens the node `kind` refers to, which the walk is in until
    /// [`Self::leave`].
    #[inline]
    fn enter(&mut self, kind: AstKind<'ast>) -> NodeId {
        let id = self.next_node;
        self.next_node = id.next();
        if let Some(nodes) = &mut self.nodes {
            nodes.push(kind, self.current_scope, self.node_stack.last().copied());
            self.node_stack.push(id);
        }
        id
    }

    #[inline]
    fn leave(&mut self) {
        if self.nodes.is_some() {
            self.node_stack.pop();
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
        self.bindings.get(name.index()).copied().unwrap_or(Binding::Free)
    }

    fn bind(&mut self, name: NameId, binding: Binding) {
        if let Some(slot) = self.bindings.get_mut(name.index()) {
            self.shadowed.push((name, mem::replace(slot, binding)));
        }
    }

    fn goto_sink(&mut self) -> Sink<'_> {
        Sink {
            scoping: &mut self.scoping,
            errors: self.is_checking.then_some(&mut self.diagnostics),
        }
    }

    fn push_scope(&mut self, kind: ScopeKind, node: NodeId) {
        let id = ScopeId::from_index(self.scoping.scopes().len());
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
    }

    fn pop_scope(&mut self) {
        let Some(outer) = self.outer_scopes.pop() else {
            return;
        };
        self.current_scope = outer.scope;
        self.current_function = outer.function;
        self.global_star = outer.global_star;
        self.named_globals = outer.named_globals;
        self.active_vars = outer.active_vars;
        if self.version.has_goto() {
            let mut gotos = mem::take(&mut self.gotos);
            gotos.close_block(outer.gotos, &mut self.goto_sink());
            self.gotos = gotos;
        }
        // Innermost first, since a scope can declare one name twice.
        let Self { shadowed, bindings, .. } = self;
        for (name, previous) in shadowed.drain(outer.shadowed_len..).rev() {
            if let Some(slot) = bindings.get_mut(name.index()) {
                *slot = previous;
            }
        }
    }

    /// The active variables where the innermost open block began, which is
    /// where a label that ends it resolves.
    fn block_start_vars(&self) -> usize {
        self.outer_scopes.last().map_or(0, |outer| outer.active_vars)
    }

    fn declare(
        &mut self,
        text: &str,
        span: Span,
        kind: SymbolKind,
        declaration: NodeId,
    ) -> SymbolId {
        let name = self.intern(text);
        let shadows = self.binding(name).local();
        let id =
            self.scoping.add_symbol(name, self.current_scope, kind, declaration, span, shadows);
        self.bind(name, Binding::Local(id));
        self.active_vars = self.active_vars.saturating_add(1);
        if self.version.has_env_upvalue() && text == "_ENV" {
            self.env_name = Some(name);
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
        self.scoping.set_role(name.index, NameRole::Symbol(id));
        self.scoping.symbol_mut(id)
    }

    /// Lua 5.5: a `global` declaration of `name`, which a value assigns when
    /// `is_assigned`.
    fn declare_global(&mut self, name: &Name<'_>, is_assigned: bool, is_read_only: bool) {
        if name.text.is_empty() {
            return;
        }
        let id = self.intern(name.text.as_str());
        self.scoping.mark_declared_global(id);
        if is_assigned {
            self.scoping.mark_assigned_global(id);
        }
        self.bind(id, if is_read_only { Binding::ReadOnlyGlobal } else { Binding::Global });
        self.named_globals = self.named_globals.saturating_add(1);
        self.active_vars = self.active_vars.saturating_add(1);
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
            && self.type_function_floor.is_some()
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
        if let Some(symbol_id) = resolved.or(env) {
            let is_captured = self.type_depth == 0
                && self.scoping.scope(self.scoping.symbol(symbol_id).scope).function
                    != self.current_function;
            if let Some(symbol) = self.scoping.symbol_mut(symbol_id) {
                symbol.is_upvalue |= is_captured;
                symbol.is_modified |= resolved.is_some() && kind.is_write();
            }
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
                if self.type_function_floor.is_some_and(|floor| scope.index() < floor) {
                    self.diagnostics.push(type_function_outer_local(name.span, text));
                }
                if let Some(until) = &mut self.until
                    && self.type_depth == 0
                    && until.scope == scope
                    && until.first_skipped <= id.index()
                    && !until.reported.contains(&id)
                {
                    until.reported.push(id);
                    let error = until_reads_skipped_local(until.condition, text);
                    self.diagnostics.push(error);
                }
                if kind.is_write() && is_read_only {
                    self.diagnostics.push(assignment_to_read_only(name.span, text));
                }
            }
            // Lua 5.5
            Binding::Global => {}
            // Lua 5.5
            Binding::ReadOnlyGlobal => {
                if kind.is_write() {
                    self.diagnostics.push(assignment_to_read_only(name.span, text));
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
                            self.diagnostics.push(assignment_to_read_only(name.span, text));
                        }
                    }
                    None => {
                        if self.named_globals > 0 {
                            self.diagnostics.push(undeclared_global(name.span, text));
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
                .map_or(0, |i| i.saturating_add(1))
        };
        for (i, stmt) in block.stmts.iter().enumerate() {
            if let Statement::Label(label) = stmt {
                let active_vars =
                    if i >= trailing_start { self.block_start_vars() } else { self.active_vars };
                self.label(label, active_vars);
            } else if let Statement::Goto(goto) = stmt {
                self.goto(goto);
            } else {
                if matches!(stmt, Statement::Continue(_)) {
                    self.mark_continue();
                }
                self.visit_stmt(stmt);
            }
        }
    }

    /// A block whose scope its parent has opened.
    fn block(&mut self, block: &'ast Block<'ast>, is_repeat_body: bool) {
        self.enter(AstKind::Block(block));
        self.walk_stmts(block, is_repeat_body);
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
        let version = self.version;
        let mut gotos = mem::take(&mut self.gotos);
        gotos.add_label(name, id, active_vars, version, &mut self.goto_sink());
        self.gotos = gotos;
    }

    // Lua 5.2+
    fn goto(&mut self, goto: &'ast GotoStatement<'ast>) {
        self.enter(AstKind::GotoStatement(goto));
        self.name_node(&goto.name);
        self.leave();
        if goto.name.text.is_empty() {
            return;
        }
        let name = self.intern(goto.name.text.as_str());
        let active_vars = self.active_vars;
        let mut gotos = mem::take(&mut self.gotos);
        gotos.add_goto(name, goto.name.index, goto.span, active_vars, &mut self.goto_sink());
        self.gotos = gotos;
    }

    // Luau
    fn mark_continue(&mut self) {
        if self.loops.len() > self.loop_base
            && let Some(frame) = self.loops.last_mut()
            && frame.is_repeat
            && frame.first_skipped.is_none()
        {
            frame.first_skipped = Some(self.scoping.symbols().len());
        }
    }

    /// A loop body, which a `continue` in it goes on with.
    fn loop_body(&mut self, block: &'ast Block<'ast>, is_scope_open: bool) {
        self.loops.push(LoopFrame { is_repeat: false, first_skipped: None });
        if is_scope_open {
            self.block(block, false);
        } else {
            self.scoped_block(ScopeKind::Loop, block);
        }
        self.loops.pop();
    }

    /// A function's signature, then the name of a `local function`, which
    /// its body sees, then, in a new function scope, its implicit `self`,
    /// its parameters, its vararg's name or Lua 5.1's implicit `arg`, and its
    /// block.
    ///
    /// Luau binds the parameters, and a `local function`'s own name, after
    /// the whole signature parses, so a `typeof` in it reads the enclosing
    /// scope.
    fn function(
        &mut self,
        body: &'ast FunctionBody<'ast>,
        own_name: Option<OwnName<'ast>>,
        is_method: bool,
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
        if let Some(own) = own_name
            && let Some(symbol) =
                self.declare_name(own.name, SymbolKind::FunctionName, own.declaration)
        {
            symbol.is_exported = own.kind.is_exported(); // Luau
            symbol.is_read_only = own.kind.is_const(); // Luau
        }

        let enclosing_gotos = if self.version.has_goto() {
            self.gotos.enter_function()
        } else {
            FunctionGotos::default()
        };
        let enclosing_loop_base = mem::replace(&mut self.loop_base, self.loops.len());
        self.push_scope(ScopeKind::Function, body_node);
        if is_method {
            let span = Span::new(body.span.start, body.span.start);
            self.declare("self", span, SymbolKind::ImplicitSelf, body_node);
        }
        for (i, param) in body.params.iter().enumerate() {
            let node = self.scratch_nodes.get(first_param.saturating_add(i)).copied();
            self.declare_name(&param.name, SymbolKind::Parameter, node.unwrap_or(body_node));
        }
        self.scratch_nodes.truncate(first_param);
        if let (Some(vararg), Some(node)) = (body.vararg.as_deref(), vararg_node) {
            // Lua 5.5: `...name` binds the vararg table to a read-only local.
            if let Some(name) = &vararg.name
                && let Some(symbol) = self.declare_name(name, SymbolKind::Parameter, node)
            {
                symbol.is_read_only = true;
            }
            // Lua 5.1: a vararg function binds its extra arguments' table to
            // `arg`, after its parameters, as `parlist` declares it.
            if self.version.has_implicit_vararg_table() {
                let span = Span::new(vararg.span.start, vararg.span.start);
                self.declare("arg", span, SymbolKind::ImplicitArg, node);
            }
        }
        self.block(&body.block, false);
        self.pop_scope();
        self.loop_base = enclosing_loop_base;
        if self.version.has_goto() {
            let mut gotos = mem::take(&mut self.gotos);
            gotos.leave_function(enclosing_gotos, &mut self.goto_sink());
            self.gotos = gotos;
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
        self.block(&chunk.block, false);
        if self.version.has_goto() {
            let mut gotos = mem::take(&mut self.gotos);
            let mut sink = self.goto_sink();
            gotos.close_block(start, &mut sink);
            gotos.leave_function(FunctionGotos::default(), &mut sink);
        }
        self.leave();
    }

    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        self.block(block, false);
    }

    // Luau
    fn visit_type(&mut self, ty: &'ast Type<'ast>) {
        self.type_depth = self.type_depth.saturating_add(1);
        self.walk_type(ty);
        self.type_depth = self.type_depth.saturating_sub(1);
    }

    fn visit_function_body(&mut self, function_body: &'ast FunctionBody<'ast>) {
        self.function(function_body, None, false);
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
            if let Some(&(node, position)) =
                self.deferred_writes.get(first_target.saturating_add(i))
            {
                self.reference(name, ReferenceKind::Write, node, Some(position));
            }
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
        for (i, attributed) in local_assignment.names.iter().enumerate() {
            let Some(&node) = self.scratch_nodes.get(first_name.saturating_add(i)) else {
                continue;
            };
            let Some(symbol) = self.declare_name(&attributed.name, SymbolKind::Local, node) else {
                continue;
            };
            let attribute = attributed.attribute.as_deref();
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
        let own = OwnName { name: &local_function.name, declaration, kind: local_function.kind };
        self.function(&local_function.body, Some(own), false);
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
        self.function(&function_decl.body, None, name.method.is_some());
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
        self.loop_body(&while_loop.block, false);
        self.leave();
    }

    /// The `until` condition is in the body's scope, so it sees the body's
    /// locals.
    fn visit_repeat_loop(&mut self, repeat_loop: &'ast RepeatLoop<'ast>) {
        let node = self.enter(AstKind::RepeatLoop(repeat_loop));
        self.push_scope(ScopeKind::Loop, node);
        let scope = self.current_scope;
        self.loops.push(LoopFrame { is_repeat: true, first_skipped: None });
        self.block(&repeat_loop.block, true);
        let skipped = self.loops.pop().and_then(|frame| frame.first_skipped);
        let until = skipped.filter(|_| self.is_checking).map(|first_skipped| UntilCheck {
            scope,
            first_skipped,
            condition: repeat_loop.condition.span(),
            reported: Vec::new(),
        });
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
        let is_read_only = self.version.has_const_for_variables();
        if let Some(symbol) =
            self.declare_name(&numeric_for.name, SymbolKind::NumericForVariable, node)
        {
            symbol.is_read_only = is_read_only; // Lua 5.5
        }
        self.loop_body(&numeric_for.block, true);
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
        let has_const_control = self.version.has_const_for_variables();
        for (i, binding) in generic_for.names.iter().enumerate() {
            let declaration = self.scratch_nodes.get(first_name.saturating_add(i)).copied();
            if let Some(symbol) = self.declare_name(
                &binding.name,
                SymbolKind::IteratorVariable,
                declaration.unwrap_or(node),
            ) {
                // Lua 5.5: only the first variable is the control variable.
                symbol.is_read_only = i == 0 && has_const_control;
            }
        }
        self.scratch_nodes.truncate(first_name);
        self.loop_body(&generic_for.block, true);
        self.pop_scope();
        self.leave();
    }

    // Lua 5.5: the initializers can't see the names being declared, and a
    // name with an initializer is assigned.
    fn visit_global_declaration(&mut self, global_declaration: &'ast GlobalDeclaration<'ast>) {
        self.enter(AstKind::GlobalDeclaration(global_declaration));
        for attributed in &global_declaration.names {
            self.visit_attributed_name(attributed);
        }
        for expr in &global_declaration.exprs {
            self.visit_expr(expr);
        }
        let is_assigned = !global_declaration.exprs.is_empty();
        for attributed in &global_declaration.names {
            let is_read_only = attributed.attribute.as_deref().is_some_and(is_read_only_attribute);
            self.declare_global(&attributed.name, is_assigned, is_read_only);
        }
        self.leave();
    }

    // Lua 5.5: declared before the body, so the body reads the global.
    fn visit_global_function(&mut self, global_function: &'ast GlobalFunction<'ast>) {
        self.enter(AstKind::GlobalFunction(global_function));
        self.name_node(&global_function.name);
        self.declare_global(&global_function.name, true, false);
        self.function(&global_function.body, None, false);
        self.leave();
    }

    // Lua 5.5
    fn visit_global_star(&mut self, global_star: &'ast GlobalStar<'ast>) {
        self.walk_global_star(global_star);
        self.global_star = Some(global_star.attribute.as_ref().is_some_and(is_read_only_attribute));
        self.active_vars = self.active_vars.saturating_add(1);
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
                let floor = Some(self.scoping.scopes().len());
                let enclosing = mem::replace(&mut self.type_function_floor, floor);
                self.function(body, None, false);
                self.type_function_floor = enclosing;
            }
        }
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use core::str;

    use luck_allocator::Allocator;
    use luck_ast::AstType;

    use super::*;
    use crate::fixtures::parse;
    use crate::id::LabelId;

    fn semantic_for<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        version: LuaVersion,
    ) -> Semantic<'ast> {
        SemanticBuilder::new()
            .with_nodes(true)
            .with_requires(true)
            .build(parse(allocator, source, version))
            .semantic
    }

    fn scoping_in(allocator: &Allocator, source: &str, version: LuaVersion) -> Scoping {
        SemanticBuilder::new().build(parse(allocator, source, version)).semantic.into_scoping()
    }

    fn scoping_of(allocator: &Allocator, source: &str) -> Scoping {
        scoping_in(allocator, source, LuaVersion::Lua54)
    }

    fn symbol<'tree>(tree: &'tree Scoping, name: &str) -> &'tree Symbol {
        tree.symbols()
            .iter()
            .find(|symbol| tree.name(symbol.name) == name)
            .unwrap_or_else(|| panic!("{name} must be declared"))
    }

    fn references<'tree>(tree: &'tree Scoping, name: &str) -> Vec<&'tree Reference> {
        tree.references().iter().filter(|reference| tree.name(reference.name) == name).collect()
    }

    fn global_names(tree: &Scoping) -> Vec<&str> {
        tree.global_references().map(|reference| tree.name(reference.name)).collect()
    }

    fn is_assigned_global(tree: &Scoping, name: &str) -> bool {
        tree.name_id(name).is_some_and(|id| tree.is_assigned_global(id))
    }

    fn is_declared_global(tree: &Scoping, name: &str) -> bool {
        tree.name_id(name).is_some_and(|id| tree.is_declared_global(id))
    }

    /// Every name node of `semantic`'s tree whose text is `text`, in source
    /// order.
    fn names_of<'ast>(semantic: &Semantic<'ast>, text: &str) -> Vec<&'ast Name<'ast>> {
        semantic
            .nodes()
            .iter()
            .filter_map(|(_, node)| node.kind().as_name())
            .filter(|name| name.text == text)
            .collect()
    }

    #[test]
    fn local_declaration_creates_symbol() {
        let allocator = Allocator::new();
        let tree = scoping_of(&allocator, "local x = 1");
        assert_eq!(tree.symbols().len(), 1);
        assert_eq!(tree.name(tree.symbols()[0].name), "x");
    }

    #[test]
    fn reference_resolves_to_local() {
        let allocator = Allocator::new();
        let tree = scoping_of(
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
        let tree = scoping_of(&allocator, "print(hello)");
        let names = global_names(&tree);
        assert!(names.contains(&"print"));
        assert!(names.contains(&"hello"));
    }

    #[test]
    fn function_body_creates_scope() {
        let allocator = Allocator::new();
        let tree = scoping_of(&allocator, "local function f(a, b) return a + b end");
        assert_eq!(tree.scopes().len(), 2);
        assert_eq!(tree.scopes()[1].kind, ScopeKind::Function);
        assert_eq!(tree.symbols().len(), 3);
        assert_eq!(symbol(&tree, "a").scope, ScopeId::from_index(1));
    }

    #[test]
    fn local_read_from_nested_function_is_upvalue() {
        let allocator = Allocator::new();
        let tree = scoping_of(
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
        let tree = scoping_of(
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
        let tree = scoping_of(
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
        let tree = scoping_of(
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
        let tree = scoping_of(
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
        let tree = scoping_of(&allocator, "for i = 1, 10 do print(i) end");
        assert_eq!(symbol(&tree, "i").kind, SymbolKind::NumericForVariable);
    }

    #[test]
    fn generic_for_declares_iterator_variables() {
        let allocator = Allocator::new();
        let tree = scoping_of(&allocator, "for k, v in pairs(t) do print(k, v) end");
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
        let tree = scoping_of(
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
        let tree = scoping_of(
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
        let tree =
            scoping_in(&allocator, "local function f(...args) return args end", LuaVersion::Lua55);
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
        let tree = scoping_of(
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
        let tree = scoping_of(
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
        let tree = scoping_of(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_of(
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
            let tree = scoping_in(
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
        let tree = scoping_of(
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
        let tree = scoping_of(
            &allocator,
            "\
_ENV = {}
print(y)",
        );
        assert!(tree.is_env_assigned());
        assert_eq!(tree.global_references().count(), 0);

        let lua51 = scoping_in(
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
        let tree = scoping_of(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_of(
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
        let tree = scoping_of(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
            &allocator,
            "\
type T = number
print(types, T)",
            LuaVersion::Luau,
        );
        assert_eq!(tree.global_references().count(), 3);
    }

    #[test]
    fn references_within_lists_a_span_s_names_in_source_order() {
        let allocator = Allocator::new();
        let source = "\
local x = 1
t[x], x = y, function() z = x end
print(x)";
        let tree = scoping_of(&allocator, source);
        let line = Span::new(12, 45);
        let names: Vec<&str> =
            tree.references_within(line).map(|id| tree.name(tree.reference(id).name)).collect();
        assert_eq!(names, ["t", "x", "x", "y", "z", "x"]);
        assert_eq!(tree.references_within(Span::new(13, 15)).count(), 1, "only `x`, not `t`");
    }

    #[test]
    fn assignment_values_are_read_before_targets_are_written() {
        let allocator = Allocator::new();
        let tree = scoping_of(
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
        let tree = scoping_of(
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
    fn type_alias_typeof_reads_enclosing_local() {
        let allocator = Allocator::new();
        let tree = scoping_in(
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
        let tree = scoping_of(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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
        let tree = scoping_in(
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

    #[test]
    fn reference_at_finds_the_name_an_offset_touches() {
        let allocator = Allocator::new();
        let tree = scoping_of(&allocator, "local ab = 1 print(ab)");
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

    const NAMES_OF_EVERY_KIND: &str = "\
local x, y = 1
print(x, t.x)
t[x], x = x, function(x) y = x.y end
function t:m(a) return self, a end
for i = 1, 2 do goto done end
::done::";

    #[test]
    fn every_name_finds_what_it_declares_refers_to_or_labels() {
        let allocator = Allocator::new();
        let semantic = semantic_for(&allocator, NAMES_OF_EVERY_KIND, LuaVersion::Lua54);
        let tree = semantic.scoping();
        let mut found = vec![0usize; tree.references().len()];
        let mut declared = Vec::new();
        for (node, row) in semantic.nodes().iter() {
            let Some(name) = row.kind().as_name() else {
                continue;
            };
            if let Some(id) = tree.reference_of(name) {
                let reference = tree.reference(id);
                assert_eq!((reference.node, reference.span), (node, name.span), "{name:?}");
                found[id.index()] += 1;
            }
            if let Some(id) = tree.symbol_of(name) {
                assert_eq!(tree.symbol(id).definition_span, name.span, "{name:?}");
                declared.push(id);
            }
            if let Some(id) = tree.label_of(name) {
                assert_eq!(tree.name(tree.label(id).name), name.text.as_str());
            }
        }
        assert!(found.iter().all(|&count| count == 1), "{found:?}");
        let spelled: Vec<SymbolId> = tree
            .symbols()
            .iter()
            .filter(|symbol| !symbol.kind.is_implicit())
            .map(|symbol| symbol.id)
            .collect();
        assert_eq!(declared, spelled);
        let goto = names_of(&semantic, "done")[0];
        assert_eq!(tree.label_of(goto), Some(LabelId::from_index(0)));
    }

    #[test]
    fn resolves_to_local_reads_the_name_s_own_reference() {
        let allocator = Allocator::new();
        let semantic = semantic_for(
            &allocator,
            "\
local x = 1
print(x)
print(y)",
            LuaVersion::Lua54,
        );
        let tree = semantic.scoping();
        let [declared, read] = names_of(&semantic, "x")[..] else {
            panic!("the source names x twice");
        };
        assert!(!tree.resolves_to_local(declared), "a declaration makes no reference");
        assert!(tree.resolves_to_local(read));
        assert!(!tree.resolves_to_local(names_of(&semantic, "y")[0]));
        assert!(tree.reaches_host_global(names_of(&semantic, "print")[0]));
    }

    #[test]
    fn lua51_binds_arg_in_every_vararg_function() {
        let allocator = Allocator::new();
        let source = "\
local arg = 1
local function f(a, ...) return arg end
local function g() return arg end
return arg";
        let semantic = semantic_for(&allocator, source, LuaVersion::Lua51);
        let tree = semantic.scoping();
        let kinds: Vec<(&str, SymbolKind)> =
            tree.symbols().iter().map(|symbol| (tree.name(symbol.name), symbol.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("arg", SymbolKind::Local),
                ("f", SymbolKind::FunctionName),
                ("a", SymbolKind::Parameter),
                ("arg", SymbolKind::ImplicitArg),
                ("g", SymbolKind::FunctionName),
            ]
        );
        let resolved: Vec<Option<SymbolId>> =
            references(tree, "arg").iter().map(|reference| reference.resolved).collect();
        assert_eq!(
            resolved,
            [
                Some(SymbolId::from_index(3)),
                Some(SymbolId::from_index(0)),
                Some(SymbolId::from_index(0)),
            ]
        );
        let implicit = tree.symbol(SymbolId::from_index(3));
        let dots = u32::try_from(source.find("...").unwrap()).unwrap();
        assert_eq!(implicit.definition_span, Span::new(dots, dots));
        assert!(matches!(semantic.nodes().kind(implicit.declaration), AstKind::VarArgParam(_)));
        for version in [LuaVersion::Lua52, LuaVersion::Luau] {
            let later = scoping_in(&allocator, source, version);
            assert!(
                references(&later, "arg")
                    .iter()
                    .all(|reference| reference.resolved == Some(SymbolId::from_index(0))),
                "{version:?}"
            );
        }
    }

    #[test]
    fn nodes_are_in_source_preorder_inside_their_parents() {
        let allocator = Allocator::new();
        let semantic = semantic_for(&allocator, NAMES_OF_EVERY_KIND, LuaVersion::Lua54);
        let nodes = semantic.nodes();
        assert!(matches!(nodes.kind(NodeId::from_index(0)), AstKind::Chunk(_)));
        assert_eq!(nodes.parent_id(NodeId::from_index(0)), None);
        let mut previous_start = 0;
        for (id, node) in nodes.iter().skip(1) {
            let span = node.span();
            assert!(previous_start <= span.start, "{id:?} starts before the node before it");
            previous_start = span.start;
            let parent = nodes.parent_id(id).expect("every node but the chunk has a parent");
            assert!(parent < id, "{id:?} comes after its parent");
            let around = nodes.get(parent).span();
            assert!(around.start <= span.start && span.end <= around.end, "{id:?} in {parent:?}");
        }
        assert_eq!(semantic.nodes().len(), {
            let unbuilt = SemanticBuilder::new()
                .build(parse(&allocator, NAMES_OF_EVERY_KIND, LuaVersion::Lua54))
                .semantic;
            assert!(unbuilt.nodes().is_empty());
            let built = semantic.scoping().references().iter().map(|reference| reference.node);
            let unbuilt_nodes =
                unbuilt.scoping().references().iter().map(|reference| reference.node);
            assert!(built.eq(unbuilt_nodes), "node ids don't depend on building the table");
            nodes.len()
        });
    }

    #[test]
    fn tables_point_at_the_nodes_that_declare_and_open() {
        let allocator = Allocator::new();
        let semantic = semantic_for(
            &allocator,
            "\
local a <const> = 1
local function f(b, ...c) end
function t:m() end
for i = 1, 2 do end
for k in pairs(t) do end
do end",
            LuaVersion::Lua55,
        );
        let (tree, nodes) = (semantic.scoping(), semantic.nodes());
        let declarations: Vec<(&str, AstType)> = tree
            .symbols()
            .iter()
            .map(|symbol| (tree.name(symbol.name), nodes.kind(symbol.declaration).ty()))
            .collect();
        assert_eq!(
            declarations,
            [
                ("a", AstType::AttributedName),
                ("f", AstType::LocalFunction),
                ("b", AstType::Parameter),
                ("c", AstType::VarArgParam),
                ("self", AstType::FunctionBody),
                ("i", AstType::NumericFor),
                ("k", AstType::Parameter),
            ]
        );
        let read_only: Vec<&str> = tree
            .symbols()
            .iter()
            .filter(|symbol| symbol.is_read_only)
            .map(|symbol| tree.name(symbol.name))
            .collect();
        assert_eq!(read_only, ["a", "c", "i", "k"]);
        let openers: Vec<AstType> =
            tree.scopes().iter().map(|scope| nodes.kind(scope.node).ty()).collect();
        assert_eq!(
            openers,
            [
                AstType::Chunk,
                AstType::FunctionBody,
                AstType::FunctionBody,
                AstType::NumericFor,
                AstType::GenericFor,
                AstType::Block,
            ]
        );
        let body = tree.scope(ScopeId::from_index(1)).node;
        assert_eq!(
            nodes.get(body).scope(),
            ScopeId::from_index(0),
            "a node is in the scope around it"
        );
        let body_block = nodes.iter().find(|&(id, _)| {
            nodes.parent_id(id) == Some(body) && matches!(nodes.kind(id), AstKind::Block(_))
        });
        assert_eq!(body_block.map(|(_, node)| node.scope()), Some(ScopeId::from_index(1)));
        assert_eq!(
            nodes.ancestor_kinds(body).map(AstKind::ty).collect::<Vec<_>>(),
            [AstType::LocalFunction, AstType::Block, AstType::Chunk,]
        );
    }

    #[test]
    fn gotos_reach_the_innermost_visible_label() {
        let allocator = Allocator::new();
        let semantic = semantic_for(
            &allocator,
            "\
::a:: ::unused::
do goto a ::a:: goto a end
goto a
local function f() ::a:: goto a end",
            LuaVersion::Lua52,
        );
        let tree = semantic.scoping();
        let targets: Vec<Option<LabelId>> =
            names_of(&semantic, "a").into_iter().map(|name| tree.label_of(name)).collect();
        let to = |i| Some(LabelId::from_index(i));
        assert_eq!(
            targets,
            [to(0), to(2), to(2), to(2), to(0), to(3), to(3)],
            "a label names itself, and each goto takes the innermost label of its function"
        );
        let used: Vec<bool> = tree.labels().iter().map(|label| label.is_used).collect();
        assert_eq!(used, [true, false, true, true]);
        assert_eq!(tree.labels()[1].scope, ScopeId::from_index(0));
        assert_eq!(tree.labels()[2].scope, ScopeId::from_index(1));
    }

    #[test]
    fn require_sites_are_the_calls_of_the_host_require() {
        let allocator = Allocator::new();
        let source = "\
local a = require(\"a\")
require \"b\"
require(x)
require(\"c\", 2)
obj:require(\"d\")
do
    local require = print
    require(\"e\")
end
_G.require(\"g\")
_ENV[\"require\"] \"h\"
_G:require(\"i\")
_G.require.j(\"k\")
return require(require(\"f\").g)";
        let semantic = semantic_for(&allocator, source, LuaVersion::Lua54);
        let sites: Vec<(&str, Option<&[u8]>)> = semantic
            .requires()
            .iter()
            .map(|site| {
                let call = site.call.span().text(source.as_bytes());
                (str::from_utf8(call).unwrap(), site.path.map(|path| path.value.as_bytes()))
            })
            .collect();
        assert_eq!(
            sites,
            [
                ("require(\"a\")", Some(&b"a"[..])),
                ("require \"b\"", Some(&b"b"[..])),
                ("require(x)", None),
                ("require(\"c\", 2)", None),
                ("_G.require(\"g\")", Some(&b"g"[..])),
                ("_ENV[\"require\"] \"h\"", Some(&b"h"[..])),
                ("require(require(\"f\").g)", None),
                ("require(\"f\")", Some(&b"f"[..])),
            ]
        );
        for shadowing in ["require = nil", "local _ENV = {}", "_ENV = {}"] {
            for call in ["require(\"a\")", "_G.require(\"a\")"] {
                let shadowed = format!("{shadowing}\n{call}");
                let found = semantic_for(&allocator, &shadowed, LuaVersion::Lua54);
                assert!(found.requires().is_empty(), "{shadowed}");
            }
        }
        let in_lua51 = semantic_for(&allocator, "_ENV.require(\"a\")", LuaVersion::Lua51);
        assert!(in_lua51.requires().is_empty(), "Lua 5.1's `_ENV` is a plain global");
        let unasked =
            SemanticBuilder::new().build(parse(&allocator, "require(\"a\")", LuaVersion::Lua54));
        assert!(unasked.semantic.requires().is_empty());
    }

    #[test]
    fn a_function_records_whether_its_own_code_reads_its_varargs() {
        let allocator = Allocator::new();
        let tree = scoping_of(
            &allocator,
            "\
local function f(...) return function(...) return ... end end
local function g(...) if ... then end end
print(...)",
        );
        let reads: Vec<bool> = tree.scopes().iter().map(|scope| scope.has_vararg_read).collect();
        assert_eq!(reads, [true, false, true, true, false], "the chunk, f, its closure, g, g's if");
    }

    #[test]
    fn a_typeof_in_a_nested_function_captures_nothing() {
        let allocator = Allocator::new();
        let tree = scoping_in(
            &allocator,
            "\
local x = 1
local function f(y: typeof(x)) local z: typeof(x) = y end",
            LuaVersion::Luau,
        );
        assert_eq!(references(&tree, "x").len(), 2);
        assert!(!symbol(&tree, "x").is_upvalue, "types never run");
    }
}
