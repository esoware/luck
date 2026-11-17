//! Hoists the declarations nested in a function body to one `local` at its
//! top, so `do local a = 1 end` becomes `local a do a = 1 end` and many such
//! declarations share one `local`.
//!
//! Runs after rename, which gives same-named bindings non-overlapping
//! lifetimes. Slot reuse can still give a child local its parent's name, so a
//! name already bound in an enclosing scope is left alone.
//!
//! There's deliberately no per-lift size check, even though a lone lift can
//! cost a few bytes for the hoisted declaration. Each lift reshapes scopes,
//! the next rename redistributes names, and that unblocks further lifts.
//! Skipping the unprofitable lone lifts stops that cascade early, and it
//! benchmarked hundreds of bytes worse across the corpus than lifting
//! everything.

use core::mem;

use luck_allocator::{Allocator, Box, List};
use luck_ast::{
    Assignment, AttributedName, Block, Expression, FunctionBody, FunctionDef, LocalAssignment,
    LocalAssignmentKind, LocalFunction, LocalFunctionKind, Name, Statement, Type, Var, VisitorMut,
};
use luck_token::{LuaVersion, MAX_ACTIVE_LOCALS, Span};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::binding::has_fixed_binding;
use crate::limits::{
    GENERIC_FOR_STATE_LOCALS, MAX_REGISTERS, NUMERIC_FOR_STATE_LOCALS, parameter_locals,
};

/// Says whether it lifted anything.
pub(crate) fn lift<'ast>(
    allocator: &'ast Allocator,
    block: &mut Block<'ast>,
    version: LuaVersion,
) -> bool {
    let mut lifter = Lifter {
        allocator,
        version,
        frames: Frames::default(),
        scans: Vec::new(),
        hidden_scan_count: 0,
        has_changed: false,
    };
    lifter.visit_block(block);
    lifter.has_changed
}

/// The names declared in each open scope, as a stack of frames, with the
/// frame of each name's innermost declaration at hand.
#[derive(Default)]
struct Frames<'ast> {
    /// Every declaration in an open frame, in order.
    log: Vec<&'ast str>,
    /// Where each open frame's declarations start in `log`.
    starts: Vec<usize>,
    /// Each name's frames, innermost last.
    by_name: FxHashMap<&'ast str, Vec<usize>>,
}

impl<'ast> Frames<'ast> {
    fn push(&mut self) {
        self.starts.push(self.log.len());
    }

    fn pop(&mut self) {
        let start = self.starts.pop().unwrap_or(0);
        for name in self.log.drain(start..) {
            if let Some(frames) = self.by_name.get_mut(name) {
                frames.pop();
            }
        }
    }

    const fn len(&self) -> usize {
        self.starts.len()
    }

    /// Declares `name` in the innermost frame.
    fn declare(&mut self, name: &'ast str) {
        let Some(frame) = self.starts.len().checked_sub(1) else {
            return;
        };
        let frames = self.by_name.entry(name).or_default();
        if frames.last() != Some(&frame) {
            frames.push(frame);
            self.log.push(name);
        }
    }

    /// The innermost frame that declares `name`.
    fn innermost(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).and_then(|frames| frames.last()).copied()
    }

    fn contains(&self, name: &str) -> bool {
        self.innermost(name).is_some()
    }
}

/// Scans every function body the walk is in, and lifts each as it leaves.
///
/// By then the body's scan has seen all of it, its closures included. A
/// closure is lifted before the body around it, whose own lift reads nothing
/// that changes.
struct Lifter<'ast> {
    allocator: &'ast Allocator,
    version: LuaVersion,
    frames: Frames<'ast>,
    /// The scan of each function body the walk is in, innermost last.
    scans: Vec<BodyScan<'ast>>,
    /// How many of `scans`, outermost first, are of bodies around a type the
    /// walk is in. Types never run, so those bodies capture no name in one
    /// and count no expression of it, and a function in a `typeof` is no
    /// closure of theirs.
    hidden_scan_count: usize,
    has_changed: bool,
}

/// What a body's walk learns for its lift: the names a declaration hoisted
/// to the top of the body must never wear, and the body's peak locals and
/// registers, which bound what that declaration can add.
///
/// The names are found by resolving scopes over the body:
///
/// - names free in the body (globals and upvalues), since the hoisted `local`
///   would capture every reference to them;
/// - names bound in the body but captured by a nested closure, since lifting
///   merges same-named bindings into one function-scope local that the
///   closure would then share;
/// - `_ENV` on targets where it's an upvalue.
///
/// A name a closure binds itself resolves inside that closure, so merely
/// mentioning a name in a closure doesn't block its lift.
struct BodyScan<'ast> {
    /// The body's parameter frame, where its scopes start.
    first_frame: usize,
    ineligible_names: FxHashSet<&'ast str>,
    /// Locals the walk has active in the body. A closure's don't count: it
    /// has its own 200.
    active_locals: usize,
    peak_active_locals: usize,
    /// Expressions seen in the body's current statement, each at most one
    /// register beyond the locals.
    statement_expr_nodes: usize,
    peak_registers: usize,
}

impl<'ast> Lifter<'ast> {
    /// The scan of the body the walk is directly in, unless a type hides it.
    fn scan(&mut self) -> Option<&mut BodyScan<'ast>> {
        if self.scans.len() > self.hidden_scan_count { self.scans.last_mut() } else { None }
    }

    fn declare(&mut self, name: &'ast str) {
        if self.scan().is_none() {
            return;
        }
        self.frames.declare(name);
        self.add_active_locals(1);
    }

    fn add_active_locals(&mut self, count: usize) {
        if let Some(scan) = self.scan() {
            scan.active_locals = scan.active_locals.saturating_add(count);
            scan.peak_active_locals = scan.peak_active_locals.max(scan.active_locals);
        }
    }

    fn push_frame(&mut self) -> Option<usize> {
        self.frames.push();
        self.scan().map(|scan| scan.active_locals)
    }

    fn pop_frame(&mut self, outer_active_locals: Option<usize>) {
        self.frames.pop();
        if let Some(active_locals) = outer_active_locals
            && let Some(scan) = self.scan()
        {
            scan.active_locals = active_locals;
        }
    }

    /// Resolves `name` once for every body that can see the read: the one
    /// that binds it, and each closure between that body and the read.
    fn reference(&mut self, name: &'ast str) {
        let scans = self.scans.get_mut(self.hidden_scan_count..).unwrap_or_default();
        let Some(outermost) = scans.first() else {
            return;
        };
        let binding_frame =
            self.frames.innermost(name).filter(|&frame| frame >= outermost.first_frame);
        let mut is_read_in_closure = false;
        for scan in scans.iter_mut().rev() {
            let is_bound_here = binding_frame.is_some_and(|frame| frame >= scan.first_frame);
            // Free in this body, or bound in it and captured.
            if !is_bound_here || is_read_in_closure {
                scan.ineligible_names.insert(name);
            }
            if is_bound_here {
                break;
            }
            is_read_in_closure = true;
        }
    }
}

impl<'ast> VisitorMut<'ast> for Lifter<'ast> {
    fn visit_block(&mut self, block: &mut Block<'ast>) {
        let outer = self.push_frame();
        self.walk_block(block);
        self.pop_frame(outer);
    }

    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        if let Some(scan) = self.scan() {
            scan.statement_expr_nodes = 0;
        }
        match stmt {
            Statement::LocalAssignment(_) => {
                // Values before names: `local x = x` reads the outer x.
                self.walk_stmt(stmt);
                if let Statement::LocalAssignment(local) = stmt {
                    for attributed in &local.names {
                        self.declare(attributed.name.text);
                    }
                }
            }
            Statement::LocalFunction(local_function) => {
                self.declare(local_function.name.text);
                self.walk_stmt(stmt);
            }
            Statement::FunctionDecl(_) => {
                self.walk_stmt(stmt);
                if let Statement::FunctionDecl(function_decl) = stmt
                    && let Some(root) = function_decl.name.names.first()
                {
                    self.reference(root.text);
                }
            }
            Statement::RepeatLoop(repeat_loop) => {
                // The condition sees the block's locals.
                let outer = self.push_frame();
                self.walk_block(&mut repeat_loop.block);
                self.visit_expr(&mut repeat_loop.condition);
                self.pop_frame(outer);
            }
            Statement::NumericFor(numeric_for) => {
                if let Some(ty) = &mut numeric_for.type_annotation {
                    self.visit_type(ty);
                }
                self.visit_expr(&mut numeric_for.start);
                self.visit_expr(&mut numeric_for.limit);
                if let Some(step) = &mut numeric_for.step {
                    self.visit_expr(step);
                }
                let outer = self.push_frame();
                self.add_active_locals(NUMERIC_FOR_STATE_LOCALS);
                self.declare(numeric_for.name.text);
                self.walk_block(&mut numeric_for.block);
                self.pop_frame(outer);
            }
            Statement::GenericFor(generic_for) => {
                for param in &mut generic_for.names {
                    if let Some(ty) = &mut param.type_annotation {
                        self.visit_type(ty);
                    }
                }
                for expr in &mut generic_for.exprs {
                    self.visit_expr(expr);
                }
                let outer = self.push_frame();
                self.add_active_locals(GENERIC_FOR_STATE_LOCALS);
                for param in &generic_for.names {
                    self.declare(param.name.text);
                }
                self.walk_block(&mut generic_for.block);
                self.pop_frame(outer);
            }
            // These bind nothing themselves, and a block of theirs opens its
            // scope in `visit_block`. A `global function` or `type function`
            // body is reached as a closure.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::IfStatement(_)
            | Statement::CompoundAssignment(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalDeclaration(_)
            | Statement::TypeDeclaration(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        if let Some(scan) = self.scan() {
            scan.statement_expr_nodes = scan.statement_expr_nodes.saturating_add(1);
            let registers = scan.active_locals.saturating_add(scan.statement_expr_nodes);
            scan.peak_registers = scan.peak_registers.max(registers);
        }
        if let Expression::Name(name) = expr {
            self.reference(name.text);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &mut Var<'ast>) {
        if let Var::Name(name) = target {
            self.reference(name.text);
            return;
        }
        self.walk_assignment_target(target);
    }

    /// The bodies the walk is already in see this one's bindings as a
    /// closure's. The signature isn't walked, so a function in one keeps
    /// its declarations.
    fn visit_function_body(&mut self, body: &mut FunctionBody<'ast>) {
        let parameter_count = parameter_locals(body);
        let mut ineligible_names = FxHashSet::default();
        if self.version.has_env_upvalue() {
            ineligible_names.insert("_ENV");
        }
        self.frames.push();
        self.scans.push(BodyScan {
            first_frame: self.frames.len().saturating_sub(1),
            ineligible_names,
            active_locals: parameter_count,
            peak_active_locals: parameter_count,
            statement_expr_nodes: 0,
            peak_registers: parameter_count,
        });
        for name in parameter_names(body) {
            self.frames.declare(name);
        }
        self.walk_block(&mut body.block);
        self.frames.pop();
        if let Some(scan) = self.scans.pop()
            && lift_body(self.allocator, body, &scan, self.version)
        {
            self.has_changed = true;
        }
    }

    fn visit_type(&mut self, ty: &mut Type<'ast>) {
        let outer_hidden_scan_count = mem::replace(&mut self.hidden_scan_count, self.scans.len());
        self.walk_type(ty);
        self.hidden_scan_count = outer_hidden_scan_count;
    }
}

/// A Lua 5.5 `...name` vararg is one too, and a hoisted declaration sharing
/// its name would capture its references.
fn parameter_names<'ast>(body: &FunctionBody<'ast>) -> impl Iterator<Item = &'ast str> {
    let vararg = body.vararg.as_ref().and_then(|vararg| vararg.name).map(|name| name.text);
    body.params.iter().map(|param| param.name.text).chain(vararg)
}

/// Hoists the declarations nested in `body` that its finished scan allows,
/// and says whether there were any.
fn lift_body<'ast>(
    allocator: &'ast Allocator,
    body: &mut FunctionBody<'ast>,
    scan: &BodyScan<'ast>,
    version: LuaVersion,
) -> bool {
    let mut collector = LiftableCollector {
        scope_names: Frames::default(),
        ineligible_names: &scan.ineligible_names,
        liftable_names: FxHashSet::default(),
        ordered_liftable_names: Vec::new(),
        // Each hoisted name adds at most one local and one register
        // anywhere in the body, so the body's own peaks bound what fits.
        hoist_capacity: MAX_ACTIVE_LOCALS
            .saturating_sub(scan.peak_active_locals)
            .min(MAX_REGISTERS.saturating_sub(scan.peak_registers)),
        version,
    };
    collector.scope_names.push();
    for name in parameter_names(body) {
        collector.scope_names.declare(name);
    }
    collector.collect_block(&body.block);
    let LiftableCollector { liftable_names, ordered_liftable_names, .. } = collector;
    // A declaration that already opens the body, lifted alone, would only
    // be fused back.
    let is_only_the_first_statement = body.block.stmts.first().is_some_and(|first| {
        let first_names: Vec<&str> = match first {
            Statement::LocalAssignment(local) => {
                local.names.iter().map(|attributed| attributed.name.text).collect()
            }
            Statement::LocalFunction(local_function) => vec![local_function.name.text],
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => Vec::new(),
        };
        first_names == ordered_liftable_names
    });
    if liftable_names.is_empty() || is_only_the_first_statement {
        return false;
    }
    let mut rewriter = LiftRewriter {
        allocator,
        liftable_names: &liftable_names,
        scope_names: Frames::default(),
        version,
    };
    rewriter.rewrite_block(&mut body.block);
    let names = ordered_liftable_names.into_iter().map(|text| AttributedName {
        name: Name { text, span: Span::new(body.span.start, body.span.start) },
        type_annotation: None,
        attribute: None,
    });
    let declaration = LocalAssignment {
        span: Span::new(body.span.start, body.span.start),
        names: List::from_iter_in(names, allocator),
        exprs: List::new(),
        kind: LocalAssignmentKind::Local,
    };
    body.block.stmts.insert(
        0,
        Statement::LocalAssignment(Box::new_in(declaration, allocator)),
        allocator,
    );
    true
}

/// A name qualifies when it isn't already bound in this or an enclosing scope
/// of the body (rename can give a child local its parent's name) and isn't
/// ineligible (see [`BodyScan`]).
///
/// A `local` statement also has to pass `can_lift_assignment`, which rules out attributes such as
/// `<const>` and `<close>`.
struct LiftableCollector<'names, 'ast> {
    scope_names: Frames<'ast>,
    ineligible_names: &'names FxHashSet<&'ast str>,
    liftable_names: FxHashSet<&'ast str>,
    ordered_liftable_names: Vec<&'ast str>,
    hoist_capacity: usize,
    version: LuaVersion,
}

impl<'ast> LiftableCollector<'_, 'ast> {
    const fn has_hoist_room(&self, name_count: usize) -> bool {
        self.ordered_liftable_names.len().saturating_add(name_count) <= self.hoist_capacity
    }

    fn is_safe_to_lift(&self, name: &str) -> bool {
        !self.scope_names.contains(name) && !self.ineligible_names.contains(name)
    }

    fn mark_liftable(&mut self, name: &'ast str) {
        if self.liftable_names.insert(name) {
            self.ordered_liftable_names.push(name);
        }
    }

    fn collect_block(&mut self, block: &Block<'ast>) {
        for stmt in &block.stmts {
            self.collect_stmt(stmt);
        }
    }

    fn collect_scoped_block(&mut self, block: &Block<'ast>, names: &[&'ast str]) {
        self.scope_names.push();
        for &name in names {
            self.scope_names.declare(name);
        }
        self.collect_block(block);
        self.scope_names.pop();
    }

    fn collect_local(&mut self, local: &LocalAssignment<'ast>) {
        if can_lift_assignment(local, self.version)
            && self.has_hoist_room(local.names.len())
            && local.names.iter().all(|attributed| self.is_safe_to_lift(attributed.name.text))
        {
            for attributed in &local.names {
                self.mark_liftable(attributed.name.text);
            }
        }
        for attributed in &local.names {
            self.scope_names.declare(attributed.name.text);
        }
    }

    fn collect_stmt(&mut self, stmt: &Statement<'ast>) {
        match stmt {
            Statement::LocalAssignment(local) => self.collect_local(local),
            Statement::LocalFunction(local_function) => {
                let name = local_function.name.text;
                if local_function.kind == LocalFunctionKind::Local
                    && self.has_hoist_room(1)
                    && self.is_safe_to_lift(name)
                {
                    self.mark_liftable(name);
                }
                self.scope_names.declare(name);
                // The function body gets its own lift, so stop here.
            }
            Statement::DoBlock(do_block) => self.collect_scoped_block(&do_block.block, &[]),
            Statement::WhileLoop(while_loop) => self.collect_scoped_block(&while_loop.block, &[]),
            Statement::RepeatLoop(repeat_loop) => {
                self.collect_scoped_block(&repeat_loop.block, &[]);
            }
            Statement::IfStatement(if_stmt) => {
                self.collect_scoped_block(&if_stmt.block, &[]);
                for clause in &if_stmt.elseif_clauses {
                    self.collect_scoped_block(&clause.block, &[]);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.collect_scoped_block(&else_clause.block, &[]);
                }
            }
            Statement::NumericFor(numeric_for) => {
                self.collect_scoped_block(&numeric_for.block, &[numeric_for.name.text]);
            }
            Statement::GenericFor(generic_for) => {
                let names: Vec<&'ast str> =
                    generic_for.names.iter().map(|param| param.name.text).collect();
                self.collect_scoped_block(&generic_for.block, &names);
            }
            // Nothing to lift at this level. Function bodies get their own
            // lift.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }
}

fn can_lift_assignment(local: &LocalAssignment<'_>, version: LuaVersion) -> bool {
    // A bare `local x` resets x to nil, which a lifted one wouldn't once a
    // sibling has used the shared slot. A type annotation would be lost,
    // since the hoisted declaration is a bare `local x`.
    !local.exprs.is_empty()
        && !has_fixed_binding(local, version)
        && local.names.iter().all(|attributed| attributed.type_annotation.is_none())
        && has_distinct_names(&local.names)
}

/// Past [`MAX_ACTIVE_LOCALS`] names, more than one function holds, none count
/// as distinct.
fn has_distinct_names(names: &[AttributedName<'_>]) -> bool {
    names.len() <= MAX_ACTIVE_LOCALS
        && names.iter().enumerate().all(|(i, attributed)| {
            names.iter().take(i).all(|earlier| earlier.name.text != attributed.name.text)
        })
}

/// Matching is by name, so it tracks every declaration in scope, lifted or
/// not. A declaration under a same-named binding stays `local`, since its
/// assignment would write to that binding instead.
struct LiftRewriter<'names, 'ast> {
    allocator: &'ast Allocator,
    liftable_names: &'names FxHashSet<&'ast str>,
    scope_names: Frames<'ast>,
    version: LuaVersion,
}

impl<'ast> LiftRewriter<'_, 'ast> {
    fn rewrite_block(&mut self, block: &mut Block<'ast>) {
        self.rewrite_scoped_block(block, &[]);
    }

    fn rewrite_scoped_block(&mut self, block: &mut Block<'ast>, names: &[&'ast str]) {
        self.scope_names.push();
        for &name in names {
            self.scope_names.declare(name);
        }
        for stmt in &mut block.stmts {
            self.rewrite_stmt(stmt);
        }
        self.scope_names.pop();
    }

    /// `local a, b = X, Y` becomes `a, b = X, Y`.
    fn rewrite_local(&mut self, stmt: &mut Statement<'ast>) {
        let Statement::LocalAssignment(local) = stmt else {
            return;
        };
        let is_lifted = can_lift_assignment(local, self.version)
            && local.names.iter().all(|attributed| {
                self.liftable_names.contains(attributed.name.text)
                    && !self.scope_names.contains(attributed.name.text)
            });
        for attributed in &local.names {
            self.scope_names.declare(attributed.name.text);
        }
        if !is_lifted {
            return;
        }
        let targets = mem::take(&mut local.names)
            .into_iter()
            .map(|attributed| Var::Name(Box::new_in(attributed.name, self.allocator)));
        let assignment = Assignment {
            span: local.span,
            targets: List::from_iter_in(targets.collect::<Vec<_>>(), self.allocator),
            values: mem::take(&mut local.exprs),
        };
        *stmt = Statement::Assignment(Box::new_in(assignment, self.allocator));
    }

    /// `local function f(x) ... end` becomes `f = function(x) ... end`.
    fn rewrite_local_function(&mut self, stmt: &mut Statement<'ast>) {
        let Statement::LocalFunction(declared) = stmt else {
            return;
        };
        let name = declared.name;
        let is_lifted = declared.kind == LocalFunctionKind::Local
            && self.liftable_names.contains(name.text)
            && !self.scope_names.contains(name.text);
        self.scope_names.declare(name.text);
        if !is_lifted {
            return;
        }
        let Statement::LocalFunction(local_function) = stmt.take() else {
            return;
        };
        let LocalFunction { span, attributes, body, .. } = local_function.unbox();
        let function = FunctionDef { span, attributes, body };
        let value = Expression::FunctionDef(Box::new_in(function, self.allocator));
        let assignment = Assignment {
            span,
            targets: List::from_array_in(
                [Var::Name(Box::new_in(name, self.allocator))],
                self.allocator,
            ),
            values: List::from_array_in([value], self.allocator),
        };
        *stmt = Statement::Assignment(Box::new_in(assignment, self.allocator));
    }

    fn rewrite_stmt(&mut self, stmt: &mut Statement<'ast>) {
        match stmt {
            Statement::LocalAssignment(_) => self.rewrite_local(stmt),
            Statement::LocalFunction(_) => self.rewrite_local_function(stmt),
            Statement::DoBlock(do_block) => self.rewrite_block(&mut do_block.block),
            Statement::WhileLoop(while_loop) => self.rewrite_block(&mut while_loop.block),
            Statement::RepeatLoop(repeat_loop) => self.rewrite_block(&mut repeat_loop.block),
            Statement::IfStatement(if_stmt) => {
                self.rewrite_block(&mut if_stmt.block);
                for clause in &mut if_stmt.elseif_clauses {
                    self.rewrite_block(&mut clause.block);
                }
                if let Some(else_clause) = &mut if_stmt.else_clause {
                    self.rewrite_block(&mut else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                let name = numeric_for.name.text;
                self.rewrite_scoped_block(&mut numeric_for.block, &[name]);
            }
            Statement::GenericFor(generic_for) => {
                let names: Vec<&'ast str> =
                    generic_for.names.iter().map(|param| param.name.text).collect();
                self.rewrite_scoped_block(&mut generic_for.block, &names);
            }
            // Nothing to rewrite at this level. Function bodies get their own
            // lift.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const LIFTING: TransformConfig = TransformConfig { lift_locals: true, ..NO_PASSES };
    const RENAMING_AND_LIFTING: TransformConfig =
        TransformConfig { rename_locals: true, rename_globals: true, ..LIFTING };

    fn apply(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &LIFTING)
    }

    fn rename_and_lift(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &RENAMING_AND_LIFTING)
    }

    fn apply_luau(source: &str) -> String {
        minify_with(source, LuaVersion::Luau, &LIFTING)
    }

    #[test]
    fn lifts_nested_locals() {
        let result = apply(
            "\
local function f(x)
  local a = 1
  if x then
    local b = 2
    print(a, b)
  end
  return a
end
return f
",
        );
        assert!(
            result.contains("local a,b"),
            "both declarations should hoist to one function-head local: {result}"
        );
        assert!(
            !result.contains("local b"),
            "the nested declaration should lose its `local`: {result}"
        );
    }

    #[test]
    fn does_not_lift_parent_shadowed() {
        // When rename gives the inner `b` the outer `a`'s name, the inner
        // declaration must keep its `local`.
        let result = rename_and_lift(
            "\
local function f()
  local a = 1
  do
    local b = 2
    print(b)
  end
  return a
end
return f
",
        );
        assert!(
            result.contains("do local l=2"),
            "the shadowed inner declaration must keep its `local`: {result}"
        );
    }

    #[test]
    fn does_not_lift_closure_in_loop() {
        // Only function bodies get lifted, so the loop needs a wrapper.
        let result = apply(
            "\
local function f()
  local t = {}
  for i = 1, 10 do
    local x = i
    t[i] = function() return x end
  end
  return t
end
return f
",
        );
        assert!(
            result.contains("local x=i"),
            "closure-captured loop local must keep its per-iteration binding: {result}"
        );
        // `local t` already opens the body, so hoisting it alone gains nothing.
        assert!(result.contains("local t={}"), "{result}");
    }

    #[test]
    fn lifts_loop_local_without_closure() {
        let result = apply(
            "\
local function f()
  local s = 0
  for i = 1, 10 do
    local x = i * 2
    s = s + x
  end
  return s
end
return f
",
        );
        assert!(
            result.contains("local s,x"),
            "an uncaptured loop local should hoist to the function head: {result}"
        );
        assert!(
            !result.contains("local x"),
            "the loop-body declaration should lose its `local`: {result}"
        );
    }

    #[test]
    fn lifts_a_multi_name_local_only_when_its_names_differ() {
        let result = apply(
            "\
local function f(c)
  if c then
    local a, b = 1, 2
    print(a, b)
  end
  if c then
    local d, d = 1, 2
    print(d)
  end
end
return f
",
        );
        assert!(result.contains("then a,b=1,2"), "distinct names should hoist: {result}");
        assert!(result.contains("local d,d=1,2"), "a repeated name should stay local: {result}");
    }

    #[test]
    fn does_not_lift_over_free_reference() {
        // `u[k]` reads the outer `u`, and the inner `local u` shadows it only
        // from its declaration on. Hoisting `local u` to the top of the body
        // would capture that read, which miscompiled roact's `Config:set`.
        let result = apply(
            "\
local u = {}
local function f(k)
  if u[k] == nil then
    local u = 1
    print(u)
  end
end
return f
",
        );
        assert!(
            result.contains("local u=1") || result.contains("local u = 1"),
            "decl sharing a free-referenced name must stay local: {result}"
        );
    }

    #[test]
    fn capture_outside_loop_still_blocks_lift() {
        let result = apply(
            "\
local function f()
  do
    local x = 1
    g = function() return x end
  end
  do
    local x = 2
    h = function() return x end
  end
end
return f
",
        );
        // Hoisting would merge both x bindings into one, letting the two
        // closures observe each other's writes.
        assert!(
            result.matches("local x").count() == 2,
            "captured locals must not merge into one hoisted slot: {result}"
        );
    }

    #[test]
    fn closure_internal_names_do_not_block_lift() {
        // The closure binds its own `x`; that must not veto lifting the
        // unrelated do-block `x`.
        let result = apply(
            "\
local function f()
  local c = function() local x = 1 return x end
  do
    local x = 2
    print(x)
  end
  return c
end
return f
",
        );
        assert!(
            !result.contains("local x=2") && !result.contains("local x = 2"),
            "closure-internal name must not block the lift: {result}"
        );
    }

    #[test]
    fn named_vararg_blocks_same_named_lift() {
        let source = "\
local function f(...args)
  do
    local args = 1
    print(args)
  end
end
return f
";
        let output = minify_with(source, LuaVersion::Lua55, &LIFTING);
        assert!(
            output.contains("local args=1") || output.contains("local args = 1"),
            "decl sharing the vararg name must stay local: {output}"
        );
    }

    #[test]
    fn shadowed_declaration_is_never_rewritten_to_assignment() {
        // The rewrite matches by name, so a declaration under a same-named
        // binding (here the `for` variable) must stay `local`, or its
        // assignment would write to that binding.
        let result = rename_and_lift(
            "\
function W.new(c)
  for l = 1, 3 do
    if c then
      local l = f()
      print(l)
    end
  end
  while c do
    local l = g()
    print(l)
  end
end
",
        );
        // The declaration under the `for` variable must stay `local`. The
        // one in the while loop may be lifted.
        assert!(
            result.contains("local l=f()") || result.contains("local l = f()"),
            "shadowed decl must stay local: {result}"
        );
    }

    #[test]
    fn hoisted_declaration_leaves_registers_for_a_wide_statement() {
        let body = (0..190u32)
            .map(|i| format!("do local v{i} = g() print(v{i}) end\n"))
            .collect::<Vec<_>>()
            .concat();
        let args = (0..100u32).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let result = apply(&format!(
            "\
local function w()
{body}f({args})
end
return w
"
        ));
        let hoisted = result.split("local ").nth(2).expect("hoisted declaration");
        let (names, _) = hoisted.split_once(' ').expect("names end at the next statement");
        let name_count = names.split(',').count();
        assert!(
            name_count + 101 <= 249,
            "{name_count} hoisted locals leave no room for the call's 101 registers"
        );
    }

    #[test]
    fn hoisted_declaration_stays_within_the_local_limit() {
        let body = (0..205u32)
            .map(|i| format!("do local v{i} = g() print(v{i}) end\n"))
            .collect::<Vec<_>>()
            .concat();
        let result = apply(&format!(
            "\
local function w()
{body}end
return w
"
        ));
        let hoisted = result.split("local ").nth(2).expect("hoisted declaration");
        let (names, _) = hoisted.split_once(' ').expect("names end at the next statement");
        let name_count = names.split(',').count();
        assert!(
            name_count <= MAX_ACTIVE_LOCALS,
            "a function may hold {MAX_ACTIVE_LOCALS} locals at once, but {name_count} were hoisted"
        );
    }

    #[test]
    fn a_read_two_closures_down_is_resolved_for_each_body_around_it() {
        let result = apply(
            "\
local function f()
  do
    local x = 1
    g = function()
      do
        local y = 2
        print(y)
      end
      return function()
        do
          local z = 3
          print(z)
        end
        return x
      end
    end
  end
end
return f
",
        );
        assert_eq!(
            result,
            "local function f()do local x=1 g=function()local y do y=2 print(y)end \
             return function()local z do z=3 print(z)end return x end end end end return f"
        );
    }

    #[test]
    fn lifts_a_function_in_a_typeof_wherever_the_walk_reaches_one() {
        let function = "function() do local b = g() print(b) end end";
        let lifted = "typeof(function()local b do b=g()print(b)end end)";
        for source in [
            format!("local v = 1 :: typeof({function})"),
            format!("local v: typeof({function}) = 1"),
            format!("type T = typeof({function})"),
            format!("type T<U = typeof({function})> = U"),
            format!("for i: typeof({function}) = 1, 2 do end"),
            format!("for k: typeof({function}) in p do end"),
            format!("f<<typeof({function})>>()"),
            format!("a:m<<typeof({function})>>()"),
            format!("local function f() local v = 1 :: typeof({function}) return v end"),
            format!("local function f(a) for i: typeof({function}) = a, 2 do end end"),
            format!("local function f(a) for k: typeof({function}) in a do end end"),
        ] {
            let result = apply_luau(&source);
            assert!(result.contains(lifted), "{source}\n{result}");
        }
        // The walk skips a signature, so a function in one keeps its `local`.
        for source in [
            format!("local function f(p: typeof({function})) end"),
            format!("local function f(): typeof({function}) end"),
        ] {
            let result = apply_luau(&source);
            assert!(result.contains("do local b=g()"), "{source}\n{result}");
        }
    }

    #[test]
    fn a_name_read_in_a_type_is_never_captured() {
        for (source, expected) in [
            (
                "\
local function f()
  do
    local x = 1
    local y = g() :: typeof(function() return x end)
    print(x, y)
  end
end
",
                "local function f()local x,y do x=1 y=g()::typeof(function()return x end)\
                 print(x,y)end end",
            ),
            (
                "\
local function f()
  do
    local x = 1
    print(x, function()
      local z = 1 :: typeof(function() return x end)
      return z
    end)
  end
end
",
                "local function f()local x do x=1 print(x,function()local z=1::\
                 typeof(function()return x end)return z end)end end",
            ),
            (
                "\
local function f()
  do
    local x = 1
    print(x)
  end
  local y = g() :: typeof(function() x = 2 end)
  return y
end
",
                "local function f()local x,y do x=1 print(x)end y=g()::\
                 typeof(function()x=2 end)return y end",
            ),
        ] {
            assert_eq!(apply_luau(source), expected, "{source}");
        }
    }

    #[test]
    fn a_function_in_a_typeof_resolves_its_own_closures() {
        let result = apply_luau(
            "\
local function f()
  do
    local x = 1
    print(x :: typeof(function()
      do
        local n = 1
        h = function() return n end
      end
      do
        local m = 2
        print(m)
      end
    end))
  end
end
",
        );
        assert_eq!(
            result,
            "local function f()local x do x=1 print(x::typeof(function()local m \
             do local n=1 h=function()return n end end do m=2 print(m)end end))end end"
        );
    }

    #[test]
    fn a_type_function_captures_like_a_closure() {
        let result = apply_luau(
            "\
local function f()
  do
    local x = 1
    print(x)
  end
  type function tf(y)
    do
      local c = y
      print(c, x)
    end
    return y
  end
end
",
        );
        assert_eq!(
            result,
            "local function f()do local x=1 print(x)end type function tf(y)local c \
             do c=y print(c,x)end return y end end"
        );
    }
}
