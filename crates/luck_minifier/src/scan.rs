//! What one round of compression learns about names before it rewrites
//! anything: which `local`s are dead, and which locals inline.
//!
//! Both work on names, not bindings. A `local` a round drops when none of its
//! names is live owns the uses in what goes with it, which count only once
//! one of its names is live, so a run of locals each used only by the next
//! goes in one round. An inlined local's name must be bound nowhere else in
//! the file and read exactly once, in its declaration's scope, so a name
//! alone finds its one read.

use core::{iter, mem};

use luck_ast::{
    Block, Expression, FunctionBody, LocalAssignment, LocalFunction, MAX_NESTING_DEPTH, Statement,
    Var, VisitorMut,
};
use luck_token::LuaVersion;

use crate::binding::{
    IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME, has_attributed_or_env_name,
    has_fixed_binding, has_implicit_arg, is_env_binding,
};
use crate::expr::is_pure_expr;
use crate::name_table::NameTable;

/// The scan's findings, kept from round to round so each scan reuses the
/// last one's allocations.
#[derive(Default)]
pub(crate) struct Scan {
    /// Each name's facts, by its id in the name table.
    names: Vec<NameFacts>,
    /// The use graph: a node per name and per droppable `local`.
    nodes: Vec<UseNode>,
    edges: Vec<UseEdge>,
    /// Live nodes whose edges aren't followed yet.
    pending: Vec<usize>,
    /// The names declared in the open scopes, in declaration order, to
    /// close them per scope.
    opened: Vec<usize>,
}

/// What the scan knows of one name.
#[derive(Clone, Copy)]
struct NameFacts {
    /// Its node in the use graph, live when code the round keeps uses it,
    /// or [`UNSEEN`] for a name the scan hasn't met.
    node: usize,
    /// How often it's read in its declaration's scope, and how deep the
    /// first of those reads sits, which for a name read once is where its
    /// value lands.
    reads: usize,
    read_depth: usize,
    /// Declared by a `local` that gives each name its own value, so a
    /// second declaration disqualifies it.
    is_declared: bool,
    /// Bound or written anywhere else, or read outside its declaration's
    /// scope.
    is_shadowed: bool,
    /// Its declaration's scope is one the walk is inside.
    is_open: bool,
}

const UNSEEN: usize = usize::MAX;

impl NameFacts {
    const UNSEEN: Self = Self {
        node: UNSEEN,
        reads: 0,
        read_depth: 0,
        is_declared: false,
        is_shadowed: false,
        is_open: false,
    };
}

impl Scan {
    /// Scans `block`, and says how many levels deep it nests, or
    /// `MAX_NESTING_DEPTH + 1` for anything deeper, where the scan stops.
    pub(crate) fn run<'ast>(
        &mut self,
        block: &mut Block<'ast>,
        names: &mut NameTable<'ast>,
        version: LuaVersion,
        should_find_dead: bool,
        should_find_inlinable: bool,
    ) -> usize {
        self.names.clear();
        self.names.resize(names.len(), NameFacts::UNSEEN);
        self.nodes.clear();
        self.edges.clear();
        self.opened.clear();
        let mut scanner = Scanner {
            scan: self,
            names,
            version,
            should_find_dead,
            should_find_inlinable,
            owner: None,
            depth: 0,
            deepest: 0,
        };
        scanner.visit_block(block);
        let deepest = scanner.deepest;
        self.propagate();
        deepest
    }

    fn facts(&self, name: Option<usize>) -> Option<&NameFacts> {
        self.names.get(name?).filter(|facts| facts.node != UNSEEN)
    }

    /// Whether code the round keeps uses the name with id `name`.
    pub(crate) fn is_live(&self, name: Option<usize>) -> bool {
        self.facts(name).is_some_and(|facts| self.is_live_node(facts.node))
    }

    /// Whether code the round keeps uses the name with id `name`, and the
    /// depth of its one read if it's a local that inlines.
    pub(crate) fn local(&self, name: Option<usize>) -> (bool, Option<usize>) {
        let Some(facts) = self.facts(name) else {
            return (false, None);
        };
        let is_inlinable = facts.reads == 1 && facts.is_declared && !facts.is_shadowed;
        (self.is_live_node(facts.node), is_inlinable.then_some(facts.read_depth))
    }

    /// The facts of the name with id `name`, met for the first time if the
    /// scan hasn't yet.
    fn entry(&mut self, name: usize) -> Option<&mut NameFacts> {
        if self.names.len() <= name {
            self.names.resize(name.saturating_add(1), NameFacts::UNSEEN);
        }
        if self.names.get(name).is_some_and(|facts| facts.node == UNSEEN) {
            let node = self.push_node();
            if let Some(facts) = self.names.get_mut(name) {
                facts.node = node;
            }
        }
        self.names.get_mut(name)
    }

    fn close_scope(&mut self, scope_start: usize) {
        for name in self.opened.drain(scope_start..) {
            if let Some(facts) = self.names.get_mut(name) {
                facts.is_open = false;
            }
        }
    }

    fn push_node(&mut self) -> usize {
        let node = self.nodes.len();
        self.nodes.push(UseNode { is_live: false, first_edge: None });
        node
    }

    fn is_live_node(&self, node: usize) -> bool {
        self.nodes.get(node).is_some_and(|state| state.is_live)
    }

    /// Makes `target` live once `node` is. Only a node that isn't live yet
    /// gains an edge, so one that goes live with none never needs following.
    fn keep(&mut self, node: usize, target: usize) {
        if self.is_live_node(node) {
            self.mark_live(target);
            return;
        }
        let edge = self.edges.len();
        if !self.is_live_node(target)
            && let Some(source) = self.nodes.get_mut(node)
        {
            self.edges.push(UseEdge { target, next: source.first_edge.replace(edge) });
        }
    }

    fn mark_live(&mut self, node: usize) {
        if let Some(state) = self.nodes.get_mut(node)
            && !mem::replace(&mut state.is_live, true)
            && state.first_edge.is_some()
        {
            self.pending.push(node);
        }
    }

    fn propagate(&mut self) {
        while let Some(node) = self.pending.pop() {
            let mut edge = self.nodes.get(node).and_then(|state| state.first_edge);
            while let Some(&UseEdge { target, next }) = edge.and_then(|index| self.edges.get(index))
            {
                self.mark_live(target);
                edge = next;
            }
        }
    }
}

/// A name, live when code the round keeps uses it, or a droppable `local`,
/// live when one of its names is.
struct UseNode {
    is_live: bool,
    first_edge: Option<usize>,
}

/// What a node keeps once it's live: a name keeps the `local`s that declare
/// it, and a `local` the names it uses. A node's edges are linked through
/// `next`.
#[derive(Clone, Copy)]
struct UseEdge {
    target: usize,
    next: Option<usize>,
}

/// What becomes of a `local` when none of its names is live.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeadLocalAction {
    Keep,
    Remove,
    /// Every initializer is pure or a call, so only the calls stay.
    ExtractCalls,
}

/// The transform can't make a value less pure, so a `local` this finds
/// droppable before a round is droppable after it.
pub(crate) fn unused_local_action(
    local: &LocalAssignment<'_>,
    version: LuaVersion,
) -> DeadLocalAction {
    // Exports, attributed names, and a 5.2+ `_ENV` are never dead.
    if local.kind.is_exported() || has_attributed_or_env_name(local, version) {
        return DeadLocalAction::Keep;
    }
    if local.exprs.iter().all(is_pure_expr) {
        DeadLocalAction::Remove
    } else if local.exprs.iter().all(|expr| is_pure_expr(expr) || is_call(expr)) {
        DeadLocalAction::ExtractCalls
    } else {
        DeadLocalAction::Keep
    }
}

pub(crate) fn is_droppable_function(
    local_function: &LocalFunction<'_>,
    version: LuaVersion,
) -> bool {
    !local_function.kind.is_exported() && !is_env_binding(local_function.name.text, version)
}

pub(crate) fn is_call(expr: &Expression<'_>) -> bool {
    matches!(expr, Expression::Suffixed(chain) if chain.is_call())
}

struct Scanner<'scan, 'ast> {
    scan: &'scan mut Scan,
    names: &'scan mut NameTable<'ast>,
    version: LuaVersion,
    should_find_dead: bool,
    should_find_inlinable: bool,
    /// The innermost droppable `local` whose dropped parts the walk is in.
    owner: Option<usize>,
    /// Levels of nesting open where the walk is, and the most there have
    /// been.
    depth: usize,
    deepest: usize,
}

impl<'ast> Scanner<'_, 'ast> {
    /// Records a read or write of `name`, which keeps it live, and for a
    /// read, `is_read`, counts it toward inlining.
    fn record(&mut self, name: &mut &'ast str, is_read: bool) {
        let id = self.names.intern(name);
        let scan = &mut *self.scan;
        let Some(&mut NameFacts { node, .. }) = scan.entry(id) else {
            return;
        };
        if self.should_find_dead {
            match self.owner {
                Some(owner) => scan.keep(owner, node),
                None => scan.mark_live(node),
            }
        }
        if !self.should_find_inlinable || !is_read {
            return;
        }
        let Some(facts) = scan.names.get_mut(id) else {
            return;
        };
        if !facts.is_open {
            facts.is_shadowed = true;
            return;
        }
        if facts.reads == 0 {
            facts.read_depth = self.depth;
        }
        facts.reads = facts.reads.saturating_add(1);
    }

    /// Makes a droppable `local` the owner, returning the one it replaces.
    fn open_owner<'name>(
        &mut self,
        names: impl Iterator<Item = &'name mut &'ast str>,
    ) -> Option<usize>
    where
        'ast: 'name,
    {
        let owner = self.scan.push_node();
        for name in names {
            let id = self.names.intern(name);
            if let Some(&mut NameFacts { node, .. }) = self.scan.entry(id) {
                self.scan.keep(node, owner);
            }
        }
        self.owner.replace(owner)
    }

    fn shadow(&mut self, name: &mut &'ast str) {
        if self.should_find_inlinable {
            let id = self.names.intern(name);
            if let Some(facts) = self.scan.entry(id) {
                facts.is_shadowed = true;
            }
        }
    }

    /// A local inlines only where the walk can tell its one read by name,
    /// and only from a statement that gives each name its own value.
    fn declare(&mut self, local: &mut LocalAssignment<'ast>) {
        if !self.should_find_inlinable {
            return;
        }
        // Bare and multi-value locals are binders too: a reference between
        // `local x` and a later `local x = 1` resolves to this one, so the
        // name can't inline.
        let can_inline =
            !has_fixed_binding(local, self.version) && local.names.len() == local.exprs.len();
        for attributed in &mut local.names {
            let id = self.names.intern(&mut attributed.name.text);
            let Some(facts) = self.scan.entry(id) else {
                continue;
            };
            if !can_inline || facts.is_declared {
                facts.is_shadowed = true;
            } else {
                facts.is_declared = true;
                facts.is_open = true;
                self.scan.opened.push(id);
            }
        }
    }

    fn visit_local(&mut self, local: &mut LocalAssignment<'ast>) {
        let is_droppable = self.should_find_dead
            && unused_local_action(local, self.version) != DeadLocalAction::Keep;
        if is_droppable {
            let outer_owner =
                self.open_owner(local.names.iter_mut().map(|attributed| &mut attributed.name.text));
            let owner = self.owner;
            self.walk_attributed_names(&mut local.names);
            for expr in &mut local.exprs {
                // A call stays when the `local` goes.
                self.owner = if is_call(expr) { outer_owner } else { owner };
                self.visit_expr(expr);
            }
            self.owner = outer_owner;
        } else {
            self.walk_attributed_names(&mut local.names);
            for expr in &mut local.exprs {
                self.visit_expr(expr);
            }
        }
        // The local's scope starts after its statement, so a read inside it
        // is another binding's.
        self.declare(local);
    }
}

impl<'ast> VisitorMut<'ast> for Scanner<'_, 'ast> {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        self.deepest = self.deepest.max(self.depth);
        self.depth <= MAX_NESTING_DEPTH
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_block(&mut self, block: &mut Block<'ast>) {
        let scope_start = self.scan.opened.len();
        self.walk_block(block);
        self.scan.close_scope(scope_start);
    }

    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        match stmt {
            Statement::LocalAssignment(local) => {
                self.visit_local(local);
                return;
            }
            Statement::LocalFunction(local_function) => {
                self.shadow(&mut local_function.name.text);
                if self.should_find_dead && is_droppable_function(local_function, self.version) {
                    let outer_owner = self.open_owner(iter::once(&mut local_function.name.text));
                    self.walk_stmt(stmt);
                    self.owner = outer_owner;
                    return;
                }
            }
            Statement::NumericFor(numeric_for) => self.shadow(&mut numeric_for.name.text),
            Statement::GenericFor(generic_for) => {
                for param in &mut generic_for.names {
                    self.shadow(&mut param.name.text);
                }
            }
            Statement::RepeatLoop(repeat_loop) => {
                // `until` reads the body's locals, so its scope closes after it.
                let scope_start = self.scan.opened.len();
                self.walk_block(&mut repeat_loop.block);
                self.visit_expr(&mut repeat_loop.condition);
                self.scan.close_scope(scope_start);
                return;
            }
            Statement::Assignment(assignment) => {
                // A write to an inlined local's name would land on a global.
                for target in &mut assignment.targets {
                    if let Var::Name(name) = target {
                        self.shadow(&mut name.text);
                    }
                }
            }
            // Luau
            Statement::CompoundAssignment(compound) => {
                if let Var::Name(name) = &mut compound.var {
                    self.shadow(&mut name.text);
                }
            }
            // `function x()` writes `x`, and `function x.y()` reads it.
            // Neither root is a name expression or an assignment target.
            Statement::FunctionDecl(function_decl) => {
                if let Some(root) = function_decl.name.names.first_mut() {
                    self.record(&mut root.text, false);
                    self.shadow(&mut root.text);
                }
                if function_decl.name.method.is_some() {
                    let mut receiver = METHOD_RECEIVER_NAME;
                    self.shadow(&mut receiver);
                }
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.shadow(&mut global_function.name.text);
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                for attributed in &mut global.names {
                    self.shadow(&mut attributed.name.text);
                }
            }
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::IfStatement(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    fn visit_function_body(&mut self, body: &mut FunctionBody<'ast>) {
        for param in &mut body.params {
            self.shadow(&mut param.name.text);
        }
        // Lua 5.5
        if let Some(vararg_name) = body.vararg.as_mut().and_then(|vararg| vararg.name.as_mut()) {
            self.shadow(&mut vararg_name.text);
        }
        if has_implicit_arg(body, self.version) {
            let mut implicit_arg = IMPLICIT_VARARG_TABLE_NAME;
            self.shadow(&mut implicit_arg);
        }
        self.walk_function_body(body);
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        if let Expression::Name(read) = expr {
            self.record(&mut read.text, true);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &mut Var<'ast>) {
        if let Var::Name(name) = target {
            self.record(&mut name.text, false);
            return;
        }
        self.walk_assignment_target(target);
    }
}
