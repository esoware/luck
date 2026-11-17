//! What one round of compression learns about names before it rewrites
//! anything: which `local`s are dead, and which locals inline.
//!
//! Both work on names, not bindings. A `local` a round drops when none of its
//! names is live owns the uses in what goes with it, which count only once
//! one of its names is live, so a run of locals each used only by the next
//! goes in one round. An inlined local's name must be bound nowhere else in
//! the file and read exactly once, in its declaration's scope, so a name
//! alone finds its one read.
//!
//! The scan after renaming also drives the lift through the tree. A lift
//! changes only what its hoisted statements tell the scan, so the scan keeps
//! what each statement it may hoist adds until its body is lifted, and then
//! records the statement as the assignment it became or as the `local` it
//! stayed.

use core::{iter, mem};

use luck_ast::{
    Block, Expression, FunctionBody, LocalAssignment, LocalFunction, LocalFunctionKind,
    MAX_NESTING_DEPTH, Statement, Type, Var, VisitorMut,
};
use luck_token::LuaVersion;

use crate::binding::{
    IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME, has_attributed_or_env_name,
    has_fixed_binding, has_implicit_arg, is_env_binding,
};
use crate::expr::is_pure_expr;
use crate::lift_locals::{Lifter, can_lift_assignment};
use crate::limits::{GENERIC_FOR_STATE_LOCALS, NUMERIC_FOR_STATE_LOCALS};
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
    /// The ids of the names of the `local`s and parameter lists the walk is
    /// in, innermost last.
    ids: Vec<usize>,
    /// What each statement the lift may hoist adds to the facts, by its
    /// candidate index, until its body is lifted, and then each of its names
    /// and whether it was.
    held: Vec<Option<HeldStatement>>,
    settled: Vec<(usize, HeldStatement, bool)>,
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

/// A statement the lift may hoist, while its body isn't lifted yet.
#[derive(Clone, Copy)]
struct HeldStatement {
    /// The node owning the uses in its values, if it's droppable. Its names
    /// keep the node live only if it stays a declaration.
    owner: Option<usize>,
    /// The node owning the statement itself, which owns those uses and the
    /// writes to its names if it becomes an assignment.
    outer_owner: Option<usize>,
}

impl Scan {
    /// Scans `block`, and says how many levels deep it nests, or
    /// `MAX_NESTING_DEPTH + 1` for anything deeper, where the scan stops.
    ///
    /// With a `lifter`, lifts every function body as the walk leaves it,
    /// and finds the facts of the tree as lifted.
    pub(crate) fn run<'ast>(
        &mut self,
        block: &mut Block<'ast>,
        names: &mut NameTable<'ast>,
        version: LuaVersion,
        should_find_dead: bool,
        should_find_inlinable: bool,
        lifter: Option<&mut Lifter<'ast>>,
    ) -> usize {
        self.names.clear();
        self.names.resize(names.len(), NameFacts::UNSEEN);
        self.nodes.clear();
        self.edges.clear();
        self.opened.clear();
        self.ids.clear();
        self.held.clear();
        let env = names.intern_text("_ENV");
        let mut scanner = Scanner {
            scan: self,
            names,
            lifter,
            lift_pauses: 0,
            env,
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

    /// The use-graph node of the name with id `name`.
    fn node(&mut self, name: usize) -> usize {
        self.entry(name).map_or(UNSEEN, |facts| facts.node)
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

    /// Makes `target` live once `keeper` is, or now when `keeper` is `None`,
    /// which code the round keeps owns. Only a node that isn't live yet
    /// gains an edge, so one that goes live with none never needs following.
    fn keep(&mut self, keeper: Option<usize>, target: usize) {
        let Some(node) = keeper.filter(|&node| !self.is_live_node(node)) else {
            self.mark_live(target);
            return;
        };
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

    /// Records a read or write under `owner` of the name with id `name`,
    /// which keeps it live once `owner` is.
    fn mark_used(&mut self, name: usize, owner: Option<usize>) {
        let node = self.node(name);
        self.keep(owner, node);
    }

    /// Makes `owner` live once one of the names of the `local` the walk is
    /// in is, whose ids start at `names_start` in `ids`.
    fn keep_by_names(&mut self, names_start: usize, owner: usize) {
        for index in names_start..self.ids.len() {
            let Some(&name) = self.ids.get(index) else {
                break;
            };
            let node = self.node(name);
            self.keep(Some(node), owner);
        }
    }

    fn hold(&mut self, candidate: usize, statement: HeldStatement) {
        if self.held.len() <= candidate {
            self.held.resize(candidate.saturating_add(1), None);
        }
        if let Some(slot) = self.held.get_mut(candidate) {
            *slot = Some(statement);
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
    lifter: Option<&'scan mut Lifter<'ast>>,
    /// How many function signatures the walk is in. The lift reads none, so
    /// it pauses in them.
    lift_pauses: usize,
    /// `_ENV`'s id.
    env: usize,
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
    /// The lift, unless there's none or the walk is in a signature.
    fn lifter(&mut self) -> Option<&mut Lifter<'ast>> {
        if self.lift_pauses == 0 { self.lifter.as_deref_mut() } else { None }
    }

    /// Records a read or write of `name`, which keeps it live, and for a
    /// read, `is_read`, counts it toward inlining. Returns its id.
    fn record(&mut self, name: &mut &'ast str, is_read: bool) -> usize {
        let id = self.names.intern(name);
        self.mark_used(id, self.owner);
        if !self.should_find_inlinable || !is_read {
            return id;
        }
        let depth = self.depth;
        let Some(facts) = self.scan.entry(id) else {
            return id;
        };
        if !facts.is_open {
            facts.is_shadowed = true;
            return id;
        }
        if facts.reads == 0 {
            facts.read_depth = depth;
        }
        facts.reads = facts.reads.saturating_add(1);
        id
    }

    /// Keeps the name with id `name` live once `owner` is.
    fn mark_used(&mut self, name: usize, owner: Option<usize>) {
        if self.should_find_dead {
            self.scan.mark_used(name, owner);
        }
    }

    /// Marks `name` as bound or written somewhere, and returns its id.
    fn shadow(&mut self, name: &mut &'ast str) -> usize {
        let id = self.names.intern(name);
        self.shadow_id(id);
        id
    }

    fn shadow_id(&mut self, name: usize) {
        if self.should_find_inlinable
            && let Some(facts) = self.scan.entry(name)
        {
            facts.is_shadowed = true;
        }
    }

    /// Makes a droppable `local` the owner, and returns it and the owner it
    /// replaces. Its names, whose ids start at `names_start` in the scan's
    /// `ids`, keep it live unless the lift may hoist it, as `candidate`, in
    /// which case they wait for the body's lift.
    fn open_owner(
        &mut self,
        names_start: usize,
        candidate: Option<usize>,
    ) -> (usize, Option<usize>) {
        let owner = self.scan.push_node();
        if let Some(index) = candidate {
            let held = HeldStatement { owner: Some(owner), outer_owner: self.owner };
            self.scan.hold(index, held);
        } else {
            self.scan.keep_by_names(names_start, owner);
        }
        (owner, self.owner.replace(owner))
    }

    /// A local inlines only where the walk can tell its one read by name,
    /// and only from a statement that gives each name its own value. Its
    /// names' ids start at `names_start` in the scan's `ids`.
    fn declare(&mut self, local: &LocalAssignment<'ast>, names_start: usize) {
        if !self.should_find_inlinable {
            return;
        }
        // Bare and multi-value locals are binders too: a reference between
        // `local x` and a later `local x = 1` resolves to this one, so the
        // name can't inline.
        let can_inline =
            !has_fixed_binding(local, self.version) && local.names.len() == local.exprs.len();
        for index in names_start..self.scan.ids.len() {
            let Some(&name) = self.scan.ids.get(index) else {
                break;
            };
            let Some(facts) = self.scan.entry(name) else {
                continue;
            };
            if !can_inline || facts.is_declared {
                facts.is_shadowed = true;
            } else {
                facts.is_declared = true;
                facts.is_open = true;
                self.scan.opened.push(name);
            }
        }
    }

    /// Declares for the lift the names whose ids start at `names_start` in
    /// the scan's `ids`.
    fn declare_for_lift(&mut self, names_start: usize) {
        if self.lift_pauses == 0
            && let Some(lifter) = self.lifter.as_deref_mut()
        {
            for &name in self.scan.ids.get(names_start..).unwrap_or_default() {
                lifter.declare(name);
            }
        }
    }

    fn visit_local(&mut self, local: &mut LocalAssignment<'ast>) {
        let ids_start = self.scan.ids.len();
        for attributed in &mut local.names {
            let id = self.names.intern(&mut attributed.name.text);
            self.scan.ids.push(id);
        }
        let candidate = if self.lift_pauses == 0
            && let Some(lifter) = self.lifter.as_deref_mut()
        {
            let is_liftable_shape = can_lift_assignment(local, self.version);
            let ids = self.scan.ids.get(ids_start..).unwrap_or_default();
            let names = ids.iter().copied().zip(local.names.iter().map(|named| named.name.text));
            lifter.record_candidate(names, is_liftable_shape)
        } else {
            None
        };
        let is_droppable = self.should_find_dead
            && unused_local_action(local, self.version) != DeadLocalAction::Keep;
        if is_droppable {
            let (owner, outer_owner) = self.open_owner(ids_start, candidate);
            self.walk_attributed_names(&mut local.names);
            for expr in &mut local.exprs {
                // A call stays when the `local` goes.
                self.owner = if is_call(expr) { outer_owner } else { Some(owner) };
                self.visit_expr(expr);
            }
            self.owner = outer_owner;
        } else {
            if let Some(index) = candidate {
                let held = HeldStatement { owner: None, outer_owner: self.owner };
                self.scan.hold(index, held);
            }
            self.walk_attributed_names(&mut local.names);
            for expr in &mut local.exprs {
                self.visit_expr(expr);
            }
        }
        // The local's scope starts after its statement, so a read inside it
        // is another binding's.
        self.declare(local, ids_start);
        self.declare_for_lift(ids_start);
        self.scan.ids.truncate(ids_start);
    }

    fn visit_local_function(&mut self, stmt: &mut Statement<'ast>) {
        let Statement::LocalFunction(local_function) = stmt else {
            return;
        };
        let name = self.shadow(&mut local_function.name.text);
        let text = local_function.name.text;
        let is_liftable_shape = local_function.kind == LocalFunctionKind::Local;
        let is_droppable =
            self.should_find_dead && is_droppable_function(local_function, self.version);
        let candidate = self.lifter().and_then(|lifter| {
            let index = lifter.record_candidate(iter::once((name, text)), is_liftable_shape);
            lifter.declare(name);
            index
        });
        if is_droppable {
            let names_start = self.scan.ids.len();
            self.scan.ids.push(name);
            let (_, outer_owner) = self.open_owner(names_start, candidate);
            self.scan.ids.truncate(names_start);
            self.walk_stmt(stmt);
            self.owner = outer_owner;
        } else {
            if let Some(index) = candidate {
                let held = HeldStatement { owner: None, outer_owner: self.owner };
                self.scan.hold(index, held);
            }
            self.walk_stmt(stmt);
        }
    }

    /// Records what the statements of the body just lifted, its candidates
    /// from `first_candidate` on, add to the facts as lifted or kept.
    fn settle_lift(&mut self, first_candidate: usize) {
        let mut settled = mem::take(&mut self.scan.settled);
        settled.clear();
        if let Some(lifter) = self.lifter.as_deref() {
            let held = self.scan.held.get(first_candidate..).unwrap_or_default();
            for (index, slot) in (first_candidate..).zip(held) {
                if let Some(statement) = *slot {
                    let is_lifted = lifter.is_lifted(index);
                    let names = lifter.names_of(index).iter();
                    settled.extend(names.map(|&(id, _)| (id, statement, is_lifted)));
                }
            }
        }
        for &(name, statement, is_lifted) in &settled {
            match (is_lifted, statement.owner) {
                // Now an assignment: its values belong to where it is, and it
                // writes its names.
                (true, values_owner) => {
                    if let Some(owner) = values_owner {
                        self.scan.keep(statement.outer_owner, owner);
                    }
                    self.mark_used(name, statement.outer_owner);
                    self.shadow_id(name);
                }
                (false, Some(owner)) => {
                    let node = self.scan.node(name);
                    self.scan.keep(Some(node), owner);
                }
                (false, None) => {}
            }
        }
        self.scan.settled = settled;
        self.scan.held.truncate(first_candidate);
        if let Some(lifter) = self.lifter.as_deref_mut() {
            lifter.finish_body(first_candidate);
        }
    }

    fn push_lift_frame(&mut self) {
        if let Some(lifter) = self.lifter() {
            lifter.push_frame();
        }
    }

    fn pop_lift_frame(&mut self) {
        if let Some(lifter) = self.lifter() {
            lifter.pop_frame();
        }
    }

    /// A loop's block, with its hidden state locals and then the loop's own
    /// names, whose ids start at `names_start` in the scan's `ids`, declared
    /// for the lift.
    fn visit_loop_block(
        &mut self,
        block: &mut Block<'ast>,
        state_locals: usize,
        names_start: usize,
    ) {
        self.push_lift_frame();
        if let Some(lifter) = self.lifter() {
            lifter.add_active_locals(state_locals);
        }
        self.declare_for_lift(names_start);
        self.scan.ids.truncate(names_start);
        self.visit_block(block);
        self.pop_lift_frame();
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
        self.push_lift_frame();
        self.walk_block(block);
        self.pop_lift_frame();
        self.scan.close_scope(scope_start);
    }

    #[expect(clippy::too_many_lines, reason = "one arm per kind of statement")]
    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        if let Some(lifter) = self.lifter() {
            lifter.start_stmt();
        }
        match stmt {
            Statement::LocalAssignment(local) => {
                self.visit_local(local);
                return;
            }
            Statement::LocalFunction(_) => {
                self.visit_local_function(stmt);
                return;
            }
            Statement::NumericFor(numeric_for) => {
                let names_start = self.scan.ids.len();
                let name = self.shadow(&mut numeric_for.name.text);
                self.scan.ids.push(name);
                if let Some(ty) = &mut numeric_for.type_annotation {
                    self.visit_type(ty);
                }
                self.visit_expr(&mut numeric_for.start);
                self.visit_expr(&mut numeric_for.limit);
                if let Some(step) = &mut numeric_for.step {
                    self.visit_expr(step);
                }
                self.visit_loop_block(
                    &mut numeric_for.block,
                    NUMERIC_FOR_STATE_LOCALS,
                    names_start,
                );
                return;
            }
            Statement::GenericFor(generic_for) => {
                let ids_start = self.scan.ids.len();
                for param in &mut generic_for.names {
                    let id = self.shadow(&mut param.name.text);
                    self.scan.ids.push(id);
                }
                for param in &mut generic_for.names {
                    if let Some(ty) = &mut param.type_annotation {
                        self.visit_type(ty);
                    }
                }
                for expr in &mut generic_for.exprs {
                    self.visit_expr(expr);
                }
                self.visit_loop_block(&mut generic_for.block, GENERIC_FOR_STATE_LOCALS, ids_start);
                return;
            }
            Statement::RepeatLoop(repeat_loop) => {
                // `until` reads the body's locals, so its scope closes after it.
                let scope_start = self.scan.opened.len();
                self.push_lift_frame();
                self.walk_block(&mut repeat_loop.block);
                self.visit_expr(&mut repeat_loop.condition);
                self.pop_lift_frame();
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
                let root = function_decl.name.names.first_mut().map(|root_name| {
                    let root = self.record(&mut root_name.text, false);
                    self.shadow_id(root);
                    root
                });
                if function_decl.name.method.is_some() {
                    let mut receiver = METHOD_RECEIVER_NAME;
                    self.shadow(&mut receiver);
                }
                self.walk_stmt(stmt);
                if let Some(root_id) = root
                    && let Some(lifter) = self.lifter()
                {
                    lifter.reference(root_id);
                }
                return;
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

    /// The lift reads no signature, so a function in one is never lifted.
    fn visit_function_body(&mut self, body: &mut FunctionBody<'ast>) {
        let ids_start = self.scan.ids.len();
        for param in &mut body.params {
            let id = self.shadow(&mut param.name.text);
            self.scan.ids.push(id);
        }
        // Lua 5.5
        if let Some(vararg_name) = body.vararg.as_mut().and_then(|vararg| vararg.name.as_mut()) {
            let id = self.shadow(&mut vararg_name.text);
            self.scan.ids.push(id);
        }
        if has_implicit_arg(body, self.version) {
            let mut implicit_arg = IMPLICIT_VARARG_TABLE_NAME;
            self.shadow(&mut implicit_arg);
        }
        if self.descend() {
            self.lift_pauses = self.lift_pauses.saturating_add(1);
            if let Some(generics) = &mut body.generics {
                self.visit_generic_type_list(generics);
            }
            for param in &mut body.params {
                if let Some(ty) = &mut param.type_annotation {
                    self.visit_type(ty);
                }
            }
            if let Some(vararg) = &mut body.vararg
                && let Some(ty) = &mut vararg.type_annotation
            {
                self.visit_type(ty);
            }
            if let Some(return_type) = &mut body.return_type {
                self.visit_type(return_type);
            }
            self.lift_pauses = self.lift_pauses.saturating_sub(1);
            let env = self.env;
            if self.lift_pauses == 0
                && let Some(lifter) = self.lifter.as_deref_mut()
            {
                let parameters = self.scan.ids.get(ids_start..).unwrap_or_default();
                lifter.enter_body(body, parameters.iter().copied(), env);
            }
            self.scan.ids.truncate(ids_start);
            self.visit_block(&mut body.block);
            if let Some(first_candidate) = self.lifter().map(|lifter| lifter.exit_body(body)) {
                self.settle_lift(first_candidate);
            }
        }
        self.ascend();
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        if let Some(lifter) = self.lifter() {
            lifter.count_expr();
        }
        if let Expression::Name(read) = expr {
            let id = self.record(&mut read.text, true);
            if let Some(lifter) = self.lifter() {
                lifter.reference(id);
            }
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &mut Var<'ast>) {
        if let Var::Name(name) = target {
            let id = self.record(&mut name.text, false);
            if let Some(lifter) = self.lifter() {
                lifter.reference(id);
            }
            return;
        }
        self.walk_assignment_target(target);
    }

    fn visit_type(&mut self, ty: &mut Type<'ast>) {
        let outer_hidden_scan_count = self.lifter().map(Lifter::enter_type);
        self.walk_type(ty);
        if let Some(hidden_scan_count) = outer_hidden_scan_count
            && let Some(lifter) = self.lifter()
        {
            lifter.exit_type(hidden_scan_count);
        }
    }
}
