//! Hoisting the declarations of a function body to one `local` at its top.
//!
//! `do local a = 1 end` becomes `local a do a = 1 end`, and declarations
//! that are never in scope together share one hoisted local. The walk lifts
//! each body as it leaves it, once nothing else changes it.
//!
//! A binding a closure references keeps its declaration, since each run of
//! that declaration gives the closure a fresh local. With renaming on, any
//! two bindings whose scopes never overlap share a hoisted local, which the
//! renamer then names. With it off, only same-named ones do, and a name the
//! file reads as a global or from a closure anywhere stays declared where it
//! is, since the hoisted local could capture that read.
//!
//! A body lifts every declaration it can or none, and only when that's
//! shorter, so lifting a body it already lifted finds nothing more.

use core::mem;

use luck_allocator::{Box, List};
use luck_ast::{
    AttributedName, Block, Expression, FunctionBody, LocalAssignment, LocalAssignmentKind,
    LocalFunction, LocalFunctionKind, Name, Statement, Type, Var, Visit,
};
use luck_token::{LuaVersion, MAX_ACTIVE_LOCALS};
use rustc_hash::FxHashMap;

use super::Compressor;
use super::merge_locals::is_bare_local;
use super::simplify_statements::ends_in_block_end;
use crate::binding::{
    Bindings, RECEIVER_NAME_ID, VARARG_TABLE_NAME_ID, has_fixed_binding, local_index, local_of,
};
use crate::limits::{
    GENERIC_FOR_STATE_LOCALS, MAX_REGISTERS, NUMERIC_FOR_STATE_LOCALS, parameter_locals,
};

/// Bytes a lifted declaration saves, its `local `, and what a `do` block
/// left scoping nothing saves once unwrapped.
const LIFTED_DECLARATION_BYTES: usize = 6;
const UNWRAPPED_DO_BYTES: usize = 6;

/// A local hoisted to the top of a body, standing for every binding lifted
/// into it.
struct Hoisted<'ast> {
    binding: usize,
    text: &'ast str,
}

/// The scratch a lift keeps from one body to the next.
#[derive(Default)]
pub(super) struct Lifter<'ast> {
    /// Each `local` and `local function` of the body being lifted, in walk
    /// order, and whether it lifts.
    declarations: Vec<bool>,
    /// The hoisted local each lifted name joins, in walk order.
    joins: Vec<usize>,
    hoisted: Vec<Hoisted<'ast>>,
    /// Hoisted locals no binding in scope holds, and those held by the
    /// bindings of the open blocks, innermost last.
    free: Vec<usize>,
    held: Vec<usize>,
    /// With names kept: each name's hoisted local, and how many bindings of
    /// each name are in scope, with the names declared in the open blocks.
    by_name: FxHashMap<usize, usize>,
    open_counts: Vec<u32>,
    open_names: Vec<usize>,
    /// `do` blocks that lifting leaves scoping nothing.
    unwrapped_blocks: usize,
    /// With names kept, each name whose reads as a global or captures kept
    /// one of the walk's lifts from a declaration.
    blocked_names: Vec<usize>,
}

/// The most locals and registers a body can hold at once.
///
/// Registers are the locals active where the body evaluates something, and
/// at most one for each expression evaluated together, outside closures,
/// types, and parentheses.
#[derive(Clone, Copy)]
struct Pressure {
    locals: usize,
    registers: usize,
}

impl Pressure {
    /// Whether `hoisted` more locals fit: each adds at most one local and one
    /// register anywhere in the body.
    const fn fits(self, hoisted: usize) -> bool {
        self.locals.saturating_add(hoisted) <= MAX_ACTIVE_LOCALS
            && self.registers.saturating_add(hoisted) <= MAX_REGISTERS
    }

    fn evaluate(&mut self, active: usize, exprs: &[&Expression<'_>]) {
        let mut counter = ExprCounter(0);
        for expr in exprs {
            counter.visit_expr(expr);
        }
        self.registers = self.registers.max(active.saturating_add(counter.0));
    }

    /// Measures `block` with `active` locals around it, and returns how many
    /// are active at its end.
    fn measure(&mut self, block: &Block<'_>, mut active: usize) -> usize {
        for stmt in &block.stmts {
            match stmt {
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::LocalAssignment(_)
                | Statement::CompoundAssignment(_)
                | Statement::FunctionDecl(_)
                | Statement::LocalFunction(_)
                | Statement::Return(_)
                | Statement::GlobalDeclaration(_)
                | Statement::GlobalFunction(_) => {
                    let mut counter = ExprCounter(0);
                    counter.visit_stmt(stmt);
                    self.registers = self.registers.max(active.saturating_add(counter.0));
                }
                Statement::DoBlock(do_block) => {
                    self.measure(&do_block.block, active);
                }
                Statement::WhileLoop(while_loop) => {
                    self.evaluate(active, &[&while_loop.condition]);
                    self.measure(&while_loop.block, active);
                }
                Statement::RepeatLoop(repeat_loop) => {
                    let inner = self.measure(&repeat_loop.block, active);
                    self.evaluate(inner, &[&repeat_loop.condition]);
                }
                Statement::IfStatement(if_stmt) => {
                    self.evaluate(active, &[&if_stmt.condition]);
                    self.measure(&if_stmt.block, active);
                    for clause in &if_stmt.elseif_clauses {
                        self.evaluate(active, &[&clause.condition]);
                        self.measure(&clause.block, active);
                    }
                    if let Some(else_clause) = &if_stmt.else_clause {
                        self.measure(&else_clause.block, active);
                    }
                }
                Statement::NumericFor(numeric_for) => {
                    let mut bounds = vec![&numeric_for.start, &numeric_for.limit];
                    bounds.extend(&numeric_for.step);
                    self.evaluate(active, &bounds);
                    let loop_active =
                        active.saturating_add(NUMERIC_FOR_STATE_LOCALS.saturating_add(1));
                    self.locals = self.locals.max(loop_active);
                    self.measure(&numeric_for.block, loop_active);
                }
                Statement::GenericFor(generic_for) => {
                    let exprs: Vec<&Expression<'_>> = generic_for.exprs.iter().collect();
                    self.evaluate(active, &exprs);
                    let loop_active = active
                        .saturating_add(GENERIC_FOR_STATE_LOCALS)
                        .saturating_add(generic_for.names.len());
                    self.locals = self.locals.max(loop_active);
                    self.measure(&generic_for.block, loop_active);
                }
                Statement::Break(_)
                | Statement::Continue(_)
                | Statement::Goto(_)
                | Statement::Label(_)
                | Statement::GlobalStar(_)
                | Statement::TypeDeclaration(_)
                | Statement::Error(_) => {}
            }
            active = active.saturating_add(super::merge_locals::declared_count(stmt));
            self.locals = self.locals.max(active);
        }
        active
    }
}

/// Counts expressions outside closures, types, and parentheses.
struct ExprCounter(usize);

impl<'tree> Visit<'tree> for ExprCounter {
    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if !matches!(expr, Expression::Parenthesized(_)) {
            self.0 = self.0.saturating_add(1);
        }
        self.walk_expr(expr);
    }

    fn visit_function_body(&mut self, _body: &'tree FunctionBody<'tree>) {}

    fn visit_type(&mut self, _ty: &'tree Type<'tree>) {}
}

/// Whether a `local` is a shape a lift can take: initialized names with no
/// attribute or annotation, all distinct where names are kept.
fn can_lift_assignment(
    local: &LocalAssignment<'_>,
    version: LuaVersion,
    keeps_names: bool,
) -> bool {
    // A bare `local x` resets x to nil, which a lifted one wouldn't once a
    // sibling has used the shared slot. A type annotation would be lost,
    // since the hoisted declaration is a bare `local x`.
    !local.exprs.is_empty()
        && !has_fixed_binding(local, version)
        && local.names.iter().all(|attributed| attributed.type_annotation.is_none())
        && (!keeps_names || has_distinct_names(&local.names))
}

/// Past [`MAX_ACTIVE_LOCALS`] names, more than one function holds, none count
/// as distinct.
fn has_distinct_names(names: &[AttributedName<'_>]) -> bool {
    names.len() <= MAX_ACTIVE_LOCALS
        && names.iter().enumerate().all(|(i, attributed)| {
            names.iter().take(i).all(|earlier| earlier.name.text != attributed.name.text)
        })
}

/// The first pass of a lift, which finds what lifts and into which hoisted
/// local.
struct Collector<'lift, 'ast> {
    lifter: &'lift mut Lifter<'ast>,
    bindings: &'lift Bindings,
    version: LuaVersion,
    keeps_names: bool,
    can_unwrap: bool,
}

impl<'ast> Collector<'_, 'ast> {
    fn name_of(&self, name: &Name<'_>) -> Option<usize> {
        let binding = self.bindings.get(local_of(name)?)?;
        usize::try_from(binding.name).ok()
    }

    /// Whether the binding `name` declares can join a hoisted local: no
    /// closure references it, and with names kept, nothing else in the
    /// file reads its name where the hoisted local would be in scope.
    fn can_lift_name(&mut self, name: &Name<'_>) -> bool {
        let Some(binding) = local_of(name).and_then(|id| self.bindings.get(id)) else {
            return false;
        };
        if binding.captures != 0 {
            return false;
        }
        if !self.keeps_names {
            return true;
        }
        let Ok(name_id) = usize::try_from(binding.name) else {
            return false;
        };
        if self.lifter.open_counts.get(name_id).is_some_and(|&count| count > 0) {
            return false;
        }
        let is_used_elsewhere =
            self.bindings.global_uses.get(name_id).is_some_and(|&uses| uses > 0)
                || self.bindings.name_captures.get(name_id).is_some_and(|&captures| captures > 0);
        if is_used_elsewhere {
            self.lifter.blocked_names.push(name_id);
        }
        !is_used_elsewhere
    }

    /// Hands the binding `name` declares the hoisted local it joins.
    fn join(&mut self, name: &Name<'ast>) {
        let Some(id) = local_of(name) else {
            return;
        };
        let name_id = self.name_of(name).filter(|_| self.keeps_names);
        let lifter = &mut *self.lifter;
        let existing = match name_id {
            Some(text_id) => lifter.by_name.get(&text_id).copied(),
            None => lifter.free.pop(),
        };
        let hoisted = existing.unwrap_or_else(|| {
            let index = lifter.hoisted.len();
            lifter.hoisted.push(Hoisted { binding: id, text: name.text.as_str() });
            index
        });
        match name_id {
            Some(text_id) => {
                lifter.by_name.insert(text_id, hoisted);
            }
            None => lifter.held.push(hoisted),
        }
        lifter.joins.push(hoisted);
    }

    /// With names kept, opens a binding of `name` where it's declared.
    fn open(&mut self, name: &Name<'_>) {
        if !self.keeps_names {
            return;
        }
        if let Some(name_id) = self.name_of(name) {
            self.open_name(name_id);
        }
    }

    fn open_name(&mut self, name_id: usize) {
        let counts = &mut self.lifter.open_counts;
        if counts.len() <= name_id {
            counts.resize(name_id.saturating_add(1), 0);
        }
        if let Some(count) = counts.get_mut(name_id) {
            *count = count.saturating_add(1);
        }
        self.lifter.open_names.push(name_id);
    }

    /// Finds what of `block` lifts, and says whether it keeps a declaration
    /// its block scopes and whether it lifts any.
    fn collect_block(&mut self, block: &Block<'ast>) -> (bool, bool) {
        let held_start = self.lifter.held.len();
        let open_start = self.lifter.open_names.len();
        let mut keeps_declaration = false;
        let mut lifts = false;
        for stmt in &block.stmts {
            match stmt {
                Statement::LocalAssignment(local) => {
                    let can_lift = can_lift_assignment(local, self.version, self.keeps_names)
                        && local
                            .names
                            .iter()
                            .all(|attributed| self.can_lift_name(&attributed.name));
                    self.lifter.declarations.push(can_lift);
                    for attributed in &local.names {
                        if can_lift {
                            self.join(&attributed.name);
                        }
                        self.open(&attributed.name);
                    }
                    lifts |= can_lift;
                    keeps_declaration |= !can_lift;
                }
                Statement::LocalFunction(local_function) => {
                    let can_lift = local_function.kind == LocalFunctionKind::Local
                        && self.can_lift_name(&local_function.name);
                    self.lifter.declarations.push(can_lift);
                    if can_lift {
                        self.join(&local_function.name);
                    }
                    self.open(&local_function.name);
                    lifts |= can_lift;
                    keeps_declaration |= !can_lift;
                }
                Statement::DoBlock(do_block) => {
                    let (keeps, lifts_inner) = self.collect_block(&do_block.block);
                    if lifts_inner
                        && !keeps
                        && self.can_unwrap
                        && !ends_in_block_end(&do_block.block, self.version)
                    {
                        self.lifter.unwrapped_blocks =
                            self.lifter.unwrapped_blocks.saturating_add(1);
                    }
                }
                Statement::WhileLoop(while_loop) => {
                    self.collect_block(&while_loop.block);
                }
                Statement::RepeatLoop(repeat_loop) => {
                    self.collect_block(&repeat_loop.block);
                }
                Statement::IfStatement(if_stmt) => {
                    self.collect_block(&if_stmt.block);
                    for clause in &if_stmt.elseif_clauses {
                        self.collect_block(&clause.block);
                    }
                    if let Some(else_clause) = &if_stmt.else_clause {
                        self.collect_block(&else_clause.block);
                    }
                }
                Statement::NumericFor(numeric_for) => {
                    let open_loop = self.lifter.open_names.len();
                    self.open(&numeric_for.name);
                    self.collect_block(&numeric_for.block);
                    self.close_names(open_loop);
                }
                Statement::GenericFor(generic_for) => {
                    let open_loop = self.lifter.open_names.len();
                    for param in &generic_for.names {
                        self.open(&param.name);
                    }
                    self.collect_block(&generic_for.block);
                    self.close_names(open_loop);
                }
                Statement::Label(_)
                | Statement::TypeDeclaration(_)
                | Statement::GlobalDeclaration(_)
                | Statement::GlobalFunction(_)
                | Statement::GlobalStar(_) => keeps_declaration = true,
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::FunctionDecl(_)
                | Statement::Return(_)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::Goto(_)
                | Statement::CompoundAssignment(_)
                | Statement::Error(_) => {}
            }
        }
        let lifter = &mut *self.lifter;
        lifter.free.extend(lifter.held.drain(held_start..).rev());
        self.close_names(open_start);
        (keeps_declaration, lifts)
    }

    fn close_names(&mut self, start: usize) {
        let lifter = &mut *self.lifter;
        for name_id in lifter.open_names.drain(start..) {
            if let Some(count) = lifter.open_counts.get_mut(name_id) {
                *count = count.saturating_sub(1);
            }
        }
    }
}

impl<'ast> Compressor<'_, 'ast> {
    /// Whether a name that kept a lift from a declaration lost the last of
    /// its reads as a global and captures later in the walk, which leaves
    /// the lift to another walk. Forgets the names, for the next walk.
    pub(super) fn take_freed_names(&mut self) -> bool {
        let bindings = &*self.bindings;
        let is_freed = self.lifter.blocked_names.iter().any(|&name_id| {
            bindings.global_uses.get(name_id).is_none_or(|&uses| uses == 0)
                && bindings.name_captures.get(name_id).is_none_or(|&captures| captures == 0)
        });
        self.lifter.blocked_names.clear();
        is_freed
    }

    /// Lifts `body` as the walk leaves it, which leaves nothing else to
    /// change in it.
    pub(super) fn lift(&mut self, body: &mut FunctionBody<'ast>) {
        let Some(hoisted_count) = self.collect(body) else {
            return;
        };
        let parameters = parameter_locals(body);
        let mut pressure = Pressure { locals: parameters, registers: parameters };
        pressure.measure(&body.block, parameters);
        if !pressure.fits(hoisted_count) {
            return;
        }
        let registers = parameter_locals(body).saturating_add(hoisted_count);
        let mut cursor = (0usize, 0usize);
        self.rewrite_block(&mut body.block, &mut cursor, registers);
        let allocator = self.allocator;
        let names = self.lifter.hoisted.iter().map(|hoisted| {
            let name =
                Name::new(hoisted.text.into(), Some(local_index(hoisted.binding)), body.span);
            AttributedName::new(name, None, None)
        });
        let declaration = Statement::new_local_assignment(
            body.span,
            List::from_iter_in(names, allocator),
            List::new(),
            LocalAssignmentKind::Local,
            allocator,
        );
        body.block.stmts.insert(0, declaration, allocator);
        if self.config.merge_locals {
            self.live_registers = parameter_locals(body);
            self.merge_stmts(&mut body.block.stmts);
        }
        self.has_changed = true;
    }

    /// Finds what of `body` lifts, and returns how many locals it hoists,
    /// if lifting it is shorter.
    fn collect(&mut self, body: &FunctionBody<'ast>) -> Option<usize> {
        let keeps_names = !self.config.rename_locals;
        let lifter = &mut self.lifter;
        lifter.declarations.clear();
        lifter.joins.clear();
        lifter.hoisted.clear();
        lifter.free.clear();
        lifter.held.clear();
        lifter.by_name.clear();
        lifter.open_names.clear();
        lifter.unwrapped_blocks = 0;
        let mut collector = Collector {
            lifter,
            bindings: &*self.bindings,
            version: self.version,
            keeps_names,
            can_unwrap: self.config.simplify_statements,
        };
        if keeps_names {
            // A method's `self` and a 5.1 vararg function's `arg` are in
            // scope everywhere in its body, and no body says whether it has
            // them.
            collector.open_name(RECEIVER_NAME_ID);
            collector.open_name(VARARG_TABLE_NAME_ID);
            for param in &body.params {
                collector.open(&param.name);
            }
            if let Some(name) = body.vararg.as_ref().and_then(|vararg| vararg.name.as_ref()) {
                collector.open(name);
            }
        }
        collector.collect_block(&body.block);
        collector.close_names(0);
        let lifted_count = self.lifter.declarations.iter().filter(|&&lifts| lifts).count();
        let hoisted = self.lifter.hoisted.len();
        let saved = lifted_count
            .saturating_mul(LIFTED_DECLARATION_BYTES)
            .saturating_add(self.lifter.unwrapped_blocks.saturating_mul(UNWRAPPED_DO_BYTES));
        // Each name and its comma, and `local ` and a separator unless the
        // names join a bare `local` that opens the body.
        let joins_first = self.config.merge_locals
            && body.block.stmts.first().is_some_and(|first| is_bare_local(first, self.version));
        let header = if joins_first { 0 } else { LIFTED_DECLARATION_BYTES };
        let cost = hoisted.saturating_mul(2).saturating_add(header);
        (lifted_count > 0 && saved > cost).then_some(hoisted)
    }

    /// Turns the lifted declarations of `block` into assignments to their
    /// hoisted locals, and unwraps and merges what that leaves, with
    /// `cursor` at the next declaration and lifted name. Says whether it
    /// lifted any.
    fn rewrite_block(
        &mut self,
        block: &mut Block<'ast>,
        cursor: &mut (usize, usize),
        outer_registers: usize,
    ) -> bool {
        let mut has_lifted = false;
        let mut registers = outer_registers;
        for stmt in &mut block.stmts {
            match stmt {
                Statement::LocalAssignment(_) | Statement::LocalFunction(_) => {
                    let lifts = self.lifter.declarations.get(cursor.0).copied().unwrap_or(false);
                    cursor.0 = cursor.0.saturating_add(1);
                    if lifts {
                        self.rewrite_declaration(stmt, cursor);
                        has_lifted = true;
                    }
                }
                Statement::DoBlock(do_block) => {
                    has_lifted |= self.rewrite_block(&mut do_block.block, cursor, registers);
                }
                Statement::WhileLoop(while_loop) => {
                    has_lifted |= self.rewrite_block(&mut while_loop.block, cursor, registers);
                }
                Statement::RepeatLoop(repeat_loop) => {
                    has_lifted |= self.rewrite_block(&mut repeat_loop.block, cursor, registers);
                }
                Statement::IfStatement(if_stmt) => {
                    has_lifted |= self.rewrite_block(&mut if_stmt.block, cursor, registers);
                    for clause in &mut if_stmt.elseif_clauses {
                        has_lifted |= self.rewrite_block(&mut clause.block, cursor, registers);
                    }
                    if let Some(else_clause) = &mut if_stmt.else_clause {
                        has_lifted |= self.rewrite_block(&mut else_clause.block, cursor, registers);
                    }
                }
                Statement::NumericFor(numeric_for) => {
                    let loop_registers =
                        registers.saturating_add(NUMERIC_FOR_STATE_LOCALS.saturating_add(1));
                    has_lifted |=
                        self.rewrite_block(&mut numeric_for.block, cursor, loop_registers);
                }
                Statement::GenericFor(generic_for) => {
                    let loop_registers = registers
                        .saturating_add(GENERIC_FOR_STATE_LOCALS)
                        .saturating_add(generic_for.names.len());
                    has_lifted |=
                        self.rewrite_block(&mut generic_for.block, cursor, loop_registers);
                }
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::FunctionDecl(_)
                | Statement::Return(_)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::Goto(_)
                | Statement::Label(_)
                | Statement::GlobalDeclaration(_)
                | Statement::GlobalFunction(_)
                | Statement::GlobalStar(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
                | Statement::Error(_) => {}
            }
            registers = registers.saturating_add(super::merge_locals::declared_count(stmt));
        }
        if has_lifted {
            if self.config.simplify_statements {
                self.unwrap_do_blocks(block);
            }
            if self.config.merge_locals {
                self.live_registers = outer_registers;
                self.merge_stmts(&mut block.stmts);
            }
        }
        has_lifted
    }

    /// `local a, b = X, Y` becomes `a, b = X, Y`, and `local function f()`
    /// becomes `f = function()`, each name now its hoisted local's, which
    /// takes the name's counts and the assignment's write.
    fn rewrite_declaration(&mut self, stmt: &mut Statement<'ast>, cursor: &mut (usize, usize)) {
        let allocator = self.allocator;
        let (names, values, span): (Vec<Name<'ast>>, _, _) = match stmt.take() {
            Statement::LocalAssignment(local) => {
                let LocalAssignment { span, names, exprs, .. } = local.unbox();
                (names.into_iter().map(|attributed| attributed.name).collect(), exprs, span)
            }
            Statement::LocalFunction(local_function) => {
                let LocalFunction { span, attributes, body, name, .. } = local_function.unbox();
                let value = Expression::new_function_def(span, attributes, body, allocator);
                (vec![name], List::from_array_in([value], allocator), span)
            }
            kept @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::Return(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_)) => {
                *stmt = kept;
                return;
            }
        };
        for name in &names {
            let joined = self.lifter.joins.get(cursor.1).copied();
            cursor.1 = cursor.1.saturating_add(1);
            let hoisted_binding = joined
                .and_then(|index| self.lifter.hoisted.get(index))
                .map(|hoisted| hoisted.binding);
            let Some(id) = local_of(name) else {
                continue;
            };
            if let Some(merged_into) = hoisted_binding.filter(|&hoisted| hoisted != id)
                && let Some(binding) = self.bindings.entries.get_mut(id)
            {
                binding.merged_into = u32::try_from(merged_into).unwrap_or(binding.merged_into);
                let (reads, writes) =
                    (mem::take(&mut binding.reads), mem::take(&mut binding.writes));
                if let Some(hoisted) = self.bindings.entries.get_mut(merged_into) {
                    hoisted.reads = hoisted.reads.saturating_add(reads);
                    hoisted.writes = hoisted.writes.saturating_add(writes);
                }
            }
            if let Some(binding) = self.bindings.get_mut(id) {
                binding.writes = binding.writes.saturating_add(1);
            }
        }
        let targets = names.into_iter().map(|name| Var::Name(Box::new_in(name, allocator)));
        *stmt = Statement::new_assignment(
            span,
            List::from_iter_in(targets, allocator),
            values,
            allocator,
        );
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaVersion, MAX_ACTIVE_LOCALS};

    use crate::fixtures::{NO_PASSES, assert_outputs, minify_with};
    use crate::transform_config::TransformConfig;

    const LIFTING: TransformConfig = TransformConfig { lift_locals: true, ..NO_PASSES };
    const RENAMING_AND_LIFTING: TransformConfig =
        TransformConfig { rename_locals: true, ..LIFTING };

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
    fn lifts_the_declarations_of_a_body_when_that_is_shorter() {
        assert_outputs(
            apply,
            &[
                (
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
                    "local function f(x)local a,b a=1 if x then b=2 print(a,b)end return a end return f",
                ),
                (
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
                    "local function f()local s,x s=0 for i=1,10 do x=i*2 s=s+x end return s end return f",
                ),
            ],
        );
    }

    #[test]
    fn keeps_the_hoisted_declaration_when_the_walk_returns_to_the_body() {
        let config = TransformConfig { remove_dead_code: true, inline_locals: true, ..LIFTING };
        let source = "\
local v = 1
if false then v = 2 end
local function g()
  do local a = x + 1 end
  do local b = x + 2 end
  return v
end
return g
";
        assert_eq!(
            minify_with(source, LuaVersion::Lua54, &config),
            "local function g()local a,b do a=x+1 end do b=x+2 end return 1 end return g"
        );
    }

    #[test]
    fn lifts_a_name_once_the_walk_drops_its_reads_as_a_global() {
        let config = TransformConfig { remove_dead_code: true, ..LIFTING };
        let source = "\
local function f()
  do local a = g() print(a) end
  do local b = g() print(b) end
end
local function h() return a end
return f
";
        assert_eq!(
            minify_with(source, LuaVersion::Lua54, &config),
            "local function f()local a,b do a=g()print(a)end do b=g()print(b)end end return f"
        );
    }

    #[test]
    fn keeps_a_lone_declaration_that_lifting_would_lengthen() {
        for source in [
            "local function f()do local b=g()print(b)end end",
            "local function f(c)local r g(function()r=1 end)if c then local a=h()print(a)end return r end",
        ] {
            assert_eq!(apply(source), source);
        }
        // A bare `local` that opens the body takes the hoisted name for its
        // comma.
        let merging = TransformConfig { merge_locals: true, ..LIFTING };
        assert_eq!(
            minify_with(
                "local function f(c)local r g(function()r=1 end)if c then local a=h()print(a)end return r end",
                LuaVersion::Lua54,
                &merging
            ),
            "local function f(c)local a,r g(function()r=1 end)if c then a=h()print(a)end return r end"
        );
    }

    #[test]
    fn declarations_never_in_scope_together_share_a_local_once_renamed() {
        let source = "\
local function f(c)
  if c then local a = g() print(a) end
  if c then local b = h() print(b) end
end
return f
";
        assert_eq!(
            apply(source),
            "local function f(c)local a,b if c then a=g()print(a)end if c then b=h()print(b)end end return f"
        );
        assert_eq!(
            rename_and_lift(source),
            "local function u(u)local l if u then l=g()print(l)end if u then l=h()print(l)end end return u"
        );
    }

    #[test]
    fn keeps_a_local_a_closure_captures() {
        // Each run of the loop body gives the closure a fresh `x`.
        let source = "\
local function f()
  local t = {}
  local n = 0
  for i = 1, 10 do
    local x = i
    t[i] = function() return x end
    local y = i * 2
    n = n + y
  end
  return t, n
end
return f
";
        assert_eq!(
            apply(source),
            "local function f()local t,n,y t={}n=0 for i=1,10 do local x=i t[i]=function()return x end \
             y=i*2 n=n+y end return t,n end return f"
        );
        assert_eq!(
            apply(
                "\
local function f()
  local function g(n) if n > 0 then return g(n - 1) end end
  local function h() return 1 end
  local k = h()
  return g, k
end
return f
"
            ),
            "local function f()local h,k local function g(n)if n>0 then return g(n-1)end end \
             h=function()return 1 end k=h()return g,k end return f"
        );
    }

    #[test]
    fn a_name_bound_inside_a_closure_blocks_nothing_outside_it() {
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
        assert!(!result.contains("local x=2"), "{result}");
    }

    #[test]
    fn a_same_named_binding_lifts_as_its_own_once_renamed() {
        let shadowing = "\
local function f()
  local a = 1
  do
    local b = 2
    print(b)
  end
  return a
end
return f
";
        assert_eq!(
            rename_and_lift(shadowing),
            "local function l()local l,u l=1 do u=2 print(u)end return l end return l"
        );
        // The declaration under the `for` variable is a binding of its own,
        // so its assignment never writes the variable.
        assert_eq!(
            rename_and_lift(
                "\
function W.new(c)
  for l = 1, 3 do
    if c then local l = f() print(l) end
  end
  while c do local l = g() print(l) end
end
"
            ),
            "function W.new(u)local l for c=1,3 do if u then l=f()print(l)end end while u do \
             l=g()print(l)end end"
        );
    }

    #[test]
    fn with_names_kept_lifts_by_name_where_the_name_is_free() {
        assert_outputs(
            apply,
            &[
                // A repeated name in one `local` stays declared.
                (
                    "\
local function f(c)
  if c then local a, b = 1, 2 print(a, b) end
  if c then local a, b = 3, 4 print(a, b) end
  if c then local d, d = 1, 2 print(d) end
end
return f
",
                    "local function f(c)local a,b if c then a,b=1,2 print(a,b)end if c then a,b=3,4 \
                     print(a,b)end if c then local d,d=1,2 print(d)end end return f",
                ),
                // `u[k]` reads the outer `u` the inner one would capture.
                (
                    "\
local u = {}
local function f(k)
  if u[k] == nil then
    local u = 1
    print(u)
  end
  do local a = 1 print(a) end
  do local b = 2 print(b) end
end
return f
",
                    "local u={}local function f(k)local a,b if u[k]==nil then local u=1 print(u)end \
                     do a=1 print(a)end do b=2 print(b)end end return f",
                ),
                // A declaration under a same-named one would write it.
                (
                    "\
function W.new(c)
  for l = 1, 3 do
    if c then local l = f() print(l) end
  end
  do local a = g() print(a) end
  do local b = g() print(b) end
end
",
                    "function W.new(c)local a,b for l=1,3 do if c then local l=f()print(l)end end \
                     do a=g()print(a)end do b=g()print(b)end end",
                ),
            ],
        );
        assert_eq!(
            minify_with(
                "\
local function f(...args)
  do local args = 1 print(args) end
  do local a = 1 print(a) end
  do local b = 2 print(b) end
end
return f
",
                LuaVersion::Lua55,
                &LIFTING
            ),
            "local function f(...args)local a,b do local args=1 print(args)end do a=1 print(a)end \
             do b=2 print(b)end end return f"
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
    fn a_read_two_closures_down_is_a_capture_for_each_body_around_it() {
        let result = apply(
            "\
local function f()
  do
    local x = 1
    g = function()
      do local y = 2 print(y) end
      do local w = 4 print(w) end
      return function()
        do local z = 3 print(z) end
        do local v = 5 print(v) end
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
            "local function f()do local x=1 g=function()local y,w do y=2 print(y)end do w=4 \
             print(w)end return function()local z,v do z=3 print(z)end do v=5 print(v)end \
             return x end end end end return f"
        );
    }

    #[test]
    fn lifts_a_function_in_a_type_wherever_it_sits() {
        let function = "function() do local b = g() print(b) end do local c = h() print(c) end end";
        let lifted = "typeof(function()local b,c do b=g()print(b)end do c=h()print(c)end end)";
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
            format!("local function f(p: typeof({function})) end"),
            format!("local function f(): typeof({function}) end"),
        ] {
            let result = apply_luau(&source);
            assert!(result.contains(lifted), "{source}\n{result}");
        }
    }

    #[test]
    fn a_read_in_a_type_captures_nothing() {
        assert_outputs(
            apply_luau,
            &[
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
  do local w = 2 print(w) end
end
",
                    "local function f()local x,w do x=1 print(x,function()local z=1::\
                     typeof(function()return x end)return z end)end do w=2 print(w)end end",
                ),
            ],
        );
    }

    #[test]
    fn a_function_in_a_type_has_closures_of_its_own() {
        let result = apply_luau(
            "\
local function f()
  do
    local x = 1
    print(x :: typeof(function()
      do local n = 1 h = function() return n end end
      do local m = 2 print(m) end
      do local o = 3 print(o) end
    end))
  end
  do local w = 2 print(w) end
end
",
        );
        assert_eq!(
            result,
            "local function f()local x,w do x=1 print(x::typeof(function()local m,o do local n=1 \
             h=function()return n end end do m=2 print(m)end do o=3 print(o)end end))end do w=2 \
             print(w)end end"
        );
    }

    #[test]
    fn a_type_function_reads_like_a_closure() {
        // `x` in the type function is a global the hoisted `x` would capture.
        let result = apply_luau(
            "\
local function f()
  do local x = 1 print(x) end
  do local w = 2 print(w) end
  type function tf(y)
    do local c = y print(c, x) end
    do local d = y print(d) end
    return y
  end
end
",
        );
        assert_eq!(
            result,
            "local function f()do local x=1 print(x)end do local w=2 print(w)end type function \
             tf(y)local c,d do c=y print(c,x)end do d=y print(d)end return y end end"
        );
    }
}
