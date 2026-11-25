//! Hoisting the declarations of a function body to one `local` at its top.
//!
//! `do local a = 1 end` becomes `local a do a = 1 end`, and declarations
//! that are never in scope together share one hoisted local. It runs once
//! the compressor's passes settle, since a lifted declaration is an
//! assignment, which no other rewrite reads as a declaration, and it lifts
//! each body as the walk leaves it. Lifting reshapes the bindings, so the
//! compressor then builds the scoping again and runs its passes on what
//! lifting leaves, where a `do` block left scoping nothing unwraps and the
//! hoisted `local` merges with its neighbors.
//!
//! A binding a closure references keeps its declaration, since each run of
//! that declaration gives the closure a fresh local. With renaming on, any
//! two bindings whose scopes never overlap share a hoisted local, which
//! takes a name the chunk uses nowhere, so the tree reads every one of them
//! as it, however they were named, and the renamer then names it. With
//! renaming off, only same-named ones do, and a name the file reads as a
//! global or from a closure anywhere stays declared where it is, since the
//! hoisted local could capture that read.
//!
//! A body lifts every declaration it can or none, and only when that's
//! shorter, so lifting a body it already lifted finds nothing more.

use std::fmt::Write as _;

use luck_allocator::{Allocator, Box, List};
use luck_ast::{
    Block, Expression, FunctionBody, LocalAssignment, LocalAssignmentKind, LocalFunction,
    LocalFunctionKind, LocalName, Name, Statement, Type, Var, Visit, VisitMut,
};
use luck_data_structures::BitSet;
use luck_semantic::{NameId, Scoping, SymbolId};
use luck_token::MAX_ACTIVE_LOCALS;
use rustc_hash::FxHashMap;

use super::Compressor;
use super::merge_locals::{declared_count, is_bare_local};
use super::simplify_statements::ends_in_block_end;
use crate::binding::{METHOD_RECEIVER_NAME, NameIndexes, has_fixed_binding};
use crate::limits::{
    MAX_REGISTERS, NUMERIC_FOR_REGISTERS, generic_for_registers, parameter_locals,
};
use crate::transform_config::TransformConfig;

/// Bytes a lifted declaration saves, its `local `, and what a `do` block
/// left scoping nothing saves once unwrapped.
const LIFTED_DECLARATION_BYTES: usize = 6;
const UNWRAPPED_DO_BYTES: usize = 6;

/// A symbol's place among the hoisted locals while it joins none.
const NOT_HOISTED: u32 = u32::MAX;

impl<'ast> Compressor<'_, 'ast> {
    /// Lifts each function body of `block`, indexing the names of the locals
    /// it hoists from `name_indexes`, and says whether it lifted any.
    pub(super) fn lift(&self, block: &mut Block<'ast>, name_indexes: &mut NameIndexes) -> bool {
        let mut lifter = Lifter::new(self.allocator, self.config, self.scoping, name_indexes);
        lifter.read_references(&self.dropped);
        lifter.visit_block(block);
        lifter.has_lifted
    }
}

/// The lift of every function body of a tree.
struct Lifter<'walk, 'ast> {
    allocator: &'ast Allocator,
    config: TransformConfig,
    scoping: &'walk Scoping,
    name_indexes: &'walk mut NameIndexes,
    /// The symbols a closure references.
    captured: BitSet,
    /// With names kept, the references to each name as a global outside
    /// types, and the captured bindings of each name.
    global_uses: Vec<u32>,
    name_captures: Vec<u32>,
    /// Each `local` and `local function` of the body being lifted, in walk
    /// order, and whether it lifts.
    declarations: Vec<bool>,
    /// The name of each local hoisted to the top of the body, which stands
    /// for every binding lifted into it.
    hoisted: Vec<&'ast str>,
    /// With renaming on, each symbol's hoisted local, or [`NOT_HOISTED`],
    /// and the symbols that have one.
    hoisted_of: Vec<u32>,
    joined: Vec<SymbolId>,
    /// Hoisted locals no binding in scope holds, and those held by the
    /// bindings of the open blocks, innermost last.
    free: Vec<usize>,
    held: Vec<usize>,
    /// With names kept: each name's hoisted local, and how many bindings of
    /// each name are in scope, with the names declared in the open blocks.
    by_name: FxHashMap<NameId, usize>,
    open_counts: Vec<u32>,
    open_names: Vec<NameId>,
    /// `do` blocks that lifting leaves scoping nothing.
    unwrapped_blocks: usize,
    /// The last of the names `_1`, `_2`, ... the lift tried for a hoisted
    /// local, and its text.
    last_fresh_name: u32,
    fresh_text: String,
    has_lifted: bool,
}

impl<'walk, 'ast> Lifter<'walk, 'ast> {
    fn new(
        allocator: &'ast Allocator,
        config: TransformConfig,
        scoping: &'walk Scoping,
        name_indexes: &'walk mut NameIndexes,
    ) -> Self {
        let (name_count, symbol_count) = if config.rename_locals {
            (0, scoping.symbols().len())
        } else {
            (scoping.names().len(), 0)
        };
        Self {
            allocator,
            config,
            scoping,
            name_indexes,
            captured: BitSet::new(scoping.symbols().len()),
            global_uses: vec![0; name_count],
            name_captures: vec![0; name_count],
            declarations: Vec::new(),
            hoisted: Vec::new(),
            hoisted_of: vec![NOT_HOISTED; symbol_count],
            joined: Vec::new(),
            free: Vec::new(),
            held: Vec::new(),
            by_name: FxHashMap::default(),
            open_counts: vec![0; name_count],
            open_names: Vec::new(),
            unwrapped_blocks: 0,
            last_fresh_name: 0,
            fresh_text: String::new(),
            has_lifted: false,
        }
    }

    const fn keeps_names(&self) -> bool {
        !self.config.rename_locals
    }

    /// Reads what each live reference outside a type tells: whether a
    /// closure captures its symbol, or with names kept, whether it reads a
    /// global.
    fn read_references(&mut self, dropped: &BitSet) {
        let scoping = self.scoping;
        for (index, reference) in scoping.references().iter().enumerate() {
            if dropped.contains(index) || reference.is_in_type {
                continue;
            }
            let Some(symbol) = reference.resolved else {
                if let Some(uses) = self.global_uses.get_mut(reference.name.index()) {
                    *uses += 1;
                }
                continue;
            };
            let declared = scoping.symbol(symbol);
            if scoping.crosses_function_boundary(reference.scope, declared.scope)
                && !self.captured.contains(symbol.index())
            {
                self.captured.insert(symbol.index());
                if let Some(captures) = self.name_captures.get_mut(declared.name.index()) {
                    *captures += 1;
                }
            }
        }
    }

    /// The name of the symbol `name` declares.
    fn name_of(&self, name: &Name<'_>) -> Option<NameId> {
        self.scoping.symbol_of(name).map(|symbol| self.scoping.symbol(symbol).name)
    }

    /// Whether the binding `name` declares can join a hoisted local: no
    /// closure references it, and with names kept, nothing else in the
    /// file reads its name where the hoisted local would be in scope.
    fn can_lift_name(&self, name: &Name<'_>) -> bool {
        let Some(symbol) = self.scoping.symbol_of(name) else {
            return false;
        };
        if self.captured.contains(symbol.index()) {
            return false;
        }
        if !self.keeps_names() {
            return true;
        }
        let name_id = self.scoping.symbol(symbol).name.index();
        self.open_counts[name_id] == 0
            && self.global_uses[name_id] == 0
            && self.name_captures[name_id] == 0
    }

    /// Whether a `local` is a shape a lift can take: initialized names with
    /// no annotation, all distinct where names are kept.
    fn can_lift_assignment(&self, local: &LocalAssignment<'_>) -> bool {
        // A bare `local x` resets x to nil, which a lifted one wouldn't once a
        // sibling has used the shared slot. A type annotation would be lost,
        // since the hoisted declaration is a bare `local x`.
        !local.exprs.is_empty()
            && !has_fixed_binding(local)
            && local.names.iter().all(|local_name| local_name.type_annotation.is_none())
            && (!self.keeps_names() || self.has_distinct_names(&local.names))
    }

    /// Past [`MAX_ACTIVE_LOCALS`] names, more than one function holds, none
    /// count as distinct.
    fn has_distinct_names(&self, names: &[LocalName<'_>]) -> bool {
        names.len() <= MAX_ACTIVE_LOCALS
            && names.iter().enumerate().all(|(i, local_name)| {
                let name = self.name_of(&local_name.name);
                names.iter().take(i).all(|earlier| self.name_of(&earlier.name) != name)
            })
    }

    /// A name no binding or global of the chunk wears, and no hoisted local
    /// the lift named before.
    fn fresh_name(&mut self) -> &'ast str {
        loop {
            self.last_fresh_name += 1;
            self.fresh_text.clear();
            let _ = write!(self.fresh_text, "_{}", self.last_fresh_name);
            if self.scoping.name_id(&self.fresh_text).is_none() {
                return self.allocator.alloc_str(&self.fresh_text);
            }
        }
    }

    /// Hands the binding `name` declares the hoisted local it joins.
    fn join(&mut self, name: &Name<'ast>) {
        let Some(symbol) = self.scoping.symbol_of(name) else {
            return;
        };
        let kept_name = self.keeps_names().then(|| self.scoping.symbol(symbol).name);
        let existing = if let Some(name_id) = kept_name {
            self.by_name.get(&name_id).copied()
        } else {
            self.free.pop()
        };
        let hoisted = existing.unwrap_or_else(|| {
            let text = if kept_name.is_some() { name.text.as_str() } else { self.fresh_name() };
            self.hoisted.push(text);
            self.hoisted.len() - 1
        });
        if let Some(name_id) = kept_name {
            self.by_name.insert(name_id, hoisted);
        } else {
            self.held.push(hoisted);
            self.hoisted_of[symbol.index()] =
                u32::try_from(hoisted).expect("a chunk hoists fewer locals than `u32::MAX`");
            self.joined.push(symbol);
        }
    }

    /// With names kept, opens a binding of `name` where it's declared.
    fn open(&mut self, name: &Name<'_>) {
        if self.keeps_names()
            && let Some(name_id) = self.name_of(name)
        {
            self.open_name(name_id);
        }
    }

    fn open_name(&mut self, name_id: NameId) {
        self.open_counts[name_id.index()] += 1;
        self.open_names.push(name_id);
    }

    /// Finds what of `block` lifts, and says whether it keeps a declaration
    /// its block scopes and whether it lifts any.
    fn collect_block(&mut self, block: &Block<'ast>) -> (bool, bool) {
        let held_start = self.held.len();
        let open_start = self.open_names.len();
        let mut keeps_declaration = false;
        let mut lifts = false;
        for stmt in &block.stmts {
            match stmt {
                Statement::LocalAssignment(local) => {
                    let can_lift = self.can_lift_assignment(local)
                        && local
                            .names
                            .iter()
                            .all(|local_name| self.can_lift_name(&local_name.name));
                    self.declarations.push(can_lift);
                    for local_name in &local.names {
                        if can_lift {
                            self.join(&local_name.name);
                        }
                        self.open(&local_name.name);
                    }
                    lifts |= can_lift;
                    keeps_declaration |= !can_lift;
                }
                Statement::LocalFunction(local_function) => {
                    let can_lift = local_function.kind == LocalFunctionKind::Local
                        && self.can_lift_name(&local_function.name);
                    self.declarations.push(can_lift);
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
                        && self.config.simplify_statements
                        && !ends_in_block_end(&do_block.block)
                    {
                        self.unwrapped_blocks += 1;
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
                    let open_loop = self.open_names.len();
                    self.open(&numeric_for.name);
                    self.collect_block(&numeric_for.block);
                    self.close_names(open_loop);
                }
                Statement::GenericFor(generic_for) => {
                    let open_loop = self.open_names.len();
                    for param in &generic_for.names {
                        self.open(&param.name);
                    }
                    self.collect_block(&generic_for.block);
                    self.close_names(open_loop);
                }
                Statement::TypeDeclaration(_) => keeps_declaration = true,
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::FunctionDecl(_)
                | Statement::Return(_)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::CompoundAssignment(_)
                | Statement::Error(_) => {}
            }
        }
        let Self { free, held, .. } = self;
        free.extend(held.drain(held_start..).rev());
        self.close_names(open_start);
        (keeps_declaration, lifts)
    }

    fn close_names(&mut self, start: usize) {
        for name_id in self.open_names.drain(start..) {
            self.open_counts[name_id.index()] -= 1;
        }
    }

    /// Finds what of `body` lifts, and returns how many locals it hoists,
    /// if lifting it is shorter.
    fn collect(&mut self, body: &FunctionBody<'ast>) -> Option<usize> {
        self.declarations.clear();
        self.hoisted.clear();
        self.free.clear();
        self.held.clear();
        self.by_name.clear();
        self.unwrapped_blocks = 0;
        if self.keeps_names() {
            // A method's `self` is in scope everywhere in its body, and no
            // body says whether it has one.
            if let Some(name_id) = self.scoping.name_id(METHOD_RECEIVER_NAME) {
                self.open_name(name_id);
            }
            for param in &body.params {
                self.open(&param.name);
            }
        }
        self.collect_block(&body.block);
        self.close_names(0);
        let lifted_count = self.declarations.iter().filter(|&&lifts| lifts).count();
        let hoisted = self.hoisted.len();
        let saved =
            lifted_count * LIFTED_DECLARATION_BYTES + self.unwrapped_blocks * UNWRAPPED_DO_BYTES;
        // Each name and its comma, and `local ` and a separator unless the
        // names join a bare `local` that opens the body.
        let joins_first =
            self.config.merge_locals && body.block.stmts.first().is_some_and(is_bare_local);
        let header = if joins_first { 0 } else { LIFTED_DECLARATION_BYTES };
        let cost = hoisted * 2 + header;
        (lifted_count > 0 && saved > cost).then_some(hoisted)
    }

    /// Lifts `body` if that's shorter and its locals and registers fit.
    fn lift(&mut self, body: &mut FunctionBody<'ast>) {
        if let Some(hoisted_count) = self.collect(body) {
            let parameters = parameter_locals(body);
            let mut pressure = Pressure { locals: parameters, registers: parameters };
            pressure.measure(&body.block, Active { locals: parameters, registers: parameters });
            if pressure.fits(hoisted_count) {
                self.rewrite(body);
            }
        }
        for symbol in self.joined.drain(..) {
            self.hoisted_of[symbol.index()] = NOT_HOISTED;
        }
    }

    /// Turns the lifted declarations of `body` into assignments to their
    /// hoisted locals and declares those at its top.
    fn rewrite(&mut self, body: &mut FunctionBody<'ast>) {
        let mut cursor = 0;
        self.rewrite_block(&mut body.block, &mut cursor);
        if !self.keeps_names() {
            Retexter {
                scoping: self.scoping,
                hoisted_of: &self.hoisted_of,
                hoisted: &self.hoisted,
            }
            .visit_function_body(body);
        }
        let allocator = self.allocator;
        let span = body.span;
        let mut names = List::with_capacity_in(self.hoisted.len(), allocator);
        for &hoisted in &self.hoisted {
            let name = Name::new(hoisted.into(), Some(self.name_indexes.next()), span);
            names.push(LocalName::new(name, None), allocator);
        }
        let declaration = Statement::new_local_assignment(
            span,
            names,
            List::new(),
            LocalAssignmentKind::Local,
            allocator,
        );
        body.block.stmts.insert(0, declaration, allocator);
        self.has_lifted = true;
    }

    /// Turns the lifted declarations of `block` into assignments to their
    /// hoisted locals, with `cursor` at the next declaration.
    fn rewrite_block(&self, block: &mut Block<'ast>, cursor: &mut usize) {
        for stmt in &mut block.stmts {
            match stmt {
                Statement::LocalAssignment(_) | Statement::LocalFunction(_) => {
                    let lifts = self.declarations[*cursor];
                    *cursor += 1;
                    if lifts {
                        self.rewrite_declaration(stmt);
                    }
                }
                Statement::DoBlock(do_block) => self.rewrite_block(&mut do_block.block, cursor),
                Statement::WhileLoop(while_loop) => {
                    self.rewrite_block(&mut while_loop.block, cursor);
                }
                Statement::RepeatLoop(repeat_loop) => {
                    self.rewrite_block(&mut repeat_loop.block, cursor);
                }
                Statement::IfStatement(if_stmt) => {
                    self.rewrite_block(&mut if_stmt.block, cursor);
                    for clause in &mut if_stmt.elseif_clauses {
                        self.rewrite_block(&mut clause.block, cursor);
                    }
                    if let Some(else_clause) = &mut if_stmt.else_clause {
                        self.rewrite_block(&mut else_clause.block, cursor);
                    }
                }
                Statement::NumericFor(numeric_for) => {
                    self.rewrite_block(&mut numeric_for.block, cursor);
                }
                Statement::GenericFor(generic_for) => {
                    self.rewrite_block(&mut generic_for.block, cursor);
                }
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::FunctionDecl(_)
                | Statement::Return(_)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
                | Statement::Error(_) => {}
            }
        }
    }

    /// `local a, b = X, Y` becomes `a, b = X, Y`, and `local function f()`
    /// becomes `f = function()`, each name its hoisted local's.
    fn rewrite_declaration(&self, stmt: &mut Statement<'ast>) {
        let allocator = self.allocator;
        let target = |name| Var::Name(Box::new_in(name, allocator));
        let (span, targets, values) = match stmt.take() {
            Statement::LocalAssignment(local) => {
                let LocalAssignment { span, names, exprs, .. } = local.unbox();
                let names = names.into_iter().map(|local_name| target(local_name.name));
                (span, List::from_iter_in(names, allocator), exprs)
            }
            Statement::LocalFunction(local_function) => {
                let LocalFunction { span, attributes, body, name, .. } = local_function.unbox();
                let value = Expression::new_function_def(span, attributes, body, allocator);
                let values = List::from_array_in([value], allocator);
                (span, List::from_array_in([target(name)], allocator), values)
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
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_)) => {
                *stmt = kept;
                return;
            }
        };
        *stmt = Statement::new_assignment(span, targets, values, allocator);
    }
}

impl<'ast> VisitMut<'ast> for Lifter<'_, 'ast> {
    fn visit_function_body(&mut self, function_body: &mut FunctionBody<'ast>) {
        self.walk_function_body(function_body);
        self.lift(function_body);
    }
}

/// Gives each name of a lifted binding its hoisted local's.
struct Retexter<'lift, 'ast> {
    scoping: &'lift Scoping,
    hoisted_of: &'lift [u32],
    hoisted: &'lift [&'ast str],
}

impl<'ast> VisitMut<'ast> for Retexter<'_, 'ast> {
    fn visit_name(&mut self, name: &mut Name<'ast>) {
        let scoping = self.scoping;
        if let Some(&hoisted) = scoping
            .symbol_of(name)
            .or_else(|| scoping.resolved_symbol_of(name))
            .and_then(|symbol| self.hoisted.get(self.hoisted_of[symbol.index()] as usize))
        {
            name.text = hoisted.into();
        }
    }
}

/// The most locals and registers a body can hold at once.
///
/// Registers are the ones active locals and `for` loops' state take where
/// the body evaluates something, and at most one for each expression
/// evaluated together, outside closures, types, and parentheses.
#[derive(Clone, Copy)]
struct Pressure {
    locals: usize,
    registers: usize,
}

/// The locals active at a point of a body, and the registers they and the
/// state of the `for` loops around it take.
#[derive(Clone, Copy)]
struct Active {
    locals: usize,
    registers: usize,
}

impl Pressure {
    /// Whether `hoisted` more locals fit: each adds at most one local and one
    /// register anywhere in the body.
    const fn fits(self, hoisted: usize) -> bool {
        self.locals + hoisted <= MAX_ACTIVE_LOCALS && self.registers + hoisted <= MAX_REGISTERS
    }

    fn evaluate<'tree>(
        &mut self,
        active: Active,
        exprs: impl IntoIterator<Item = &'tree Expression<'tree>>,
    ) {
        let mut counter = ExprCounter(0);
        for expr in exprs {
            counter.visit_expr(expr);
        }
        self.registers = self.registers.max(active.registers + counter.0);
    }

    /// Measures `block` with `active` around it, and returns what's active
    /// at its end.
    fn measure(&mut self, block: &Block<'_>, mut active: Active) -> Active {
        for stmt in &block.stmts {
            match stmt {
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::LocalAssignment(_)
                | Statement::CompoundAssignment(_)
                | Statement::FunctionDecl(_)
                | Statement::LocalFunction(_)
                | Statement::Return(_) => {
                    let mut counter = ExprCounter(0);
                    counter.visit_stmt(stmt);
                    self.registers = self.registers.max(active.registers + counter.0);
                }
                Statement::DoBlock(do_block) => {
                    self.measure(&do_block.block, active);
                }
                Statement::WhileLoop(while_loop) => {
                    self.evaluate(active, [&while_loop.condition]);
                    self.measure(&while_loop.block, active);
                }
                Statement::RepeatLoop(repeat_loop) => {
                    let inner = self.measure(&repeat_loop.block, active);
                    self.evaluate(inner, [&repeat_loop.condition]);
                }
                Statement::IfStatement(if_stmt) => {
                    self.evaluate(active, [&if_stmt.condition]);
                    self.measure(&if_stmt.block, active);
                    for clause in &if_stmt.elseif_clauses {
                        self.evaluate(active, [&clause.condition]);
                        self.measure(&clause.block, active);
                    }
                    if let Some(else_clause) = &if_stmt.else_clause {
                        self.measure(&else_clause.block, active);
                    }
                }
                Statement::NumericFor(numeric_for) => {
                    let bounds = [&numeric_for.start, &numeric_for.limit];
                    self.evaluate(active, bounds.into_iter().chain(&numeric_for.step));
                    let loop_active = Active {
                        locals: active.locals + 1,
                        registers: active.registers + NUMERIC_FOR_REGISTERS,
                    };
                    self.locals = self.locals.max(loop_active.locals);
                    self.measure(&numeric_for.block, loop_active);
                }
                Statement::GenericFor(generic_for) => {
                    self.evaluate(active, &generic_for.exprs);
                    let names = generic_for.names.len();
                    let loop_active = Active {
                        locals: active.locals + names,
                        registers: active.registers + generic_for_registers(names),
                    };
                    self.locals = self.locals.max(loop_active.locals);
                    self.measure(&generic_for.block, loop_active);
                }
                Statement::Break(_)
                | Statement::Continue(_)
                | Statement::TypeDeclaration(_)
                | Statement::Error(_) => {}
            }
            let declared = declared_count(stmt);
            active =
                Active { locals: active.locals + declared, registers: active.registers + declared };
            self.locals = self.locals.max(active.locals);
        }
        active
    }
}

/// Counts expressions outside closures, types, and parentheses.
struct ExprCounter(usize);

impl<'tree> Visit<'tree> for ExprCounter {
    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if !matches!(expr, Expression::Parenthesized(_)) {
            self.0 += 1;
        }
        self.walk_expr(expr);
    }

    fn visit_function_body(&mut self, _body: &'tree FunctionBody<'tree>) {}

    fn visit_type(&mut self, _ty: &'tree Type<'tree>) {}
}

#[cfg(test)]
mod tests {
    use luck_token::MAX_ACTIVE_LOCALS;

    use crate::fixtures::{NO_PASSES, assert_outputs, minify_with};
    use crate::limits::MAX_REGISTERS;
    use crate::transform_config::TransformConfig;

    const LIFTING: TransformConfig = TransformConfig { lift_locals: true, ..NO_PASSES };
    const RENAMING_AND_LIFTING: TransformConfig =
        TransformConfig { rename_locals: true, ..LIFTING };

    fn apply(source: &str) -> String {
        minify_with(source, &LIFTING)
    }

    fn rename_and_lift(source: &str) -> String {
        minify_with(source, &RENAMING_AND_LIFTING)
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
            minify_with(source, &config),
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
            minify_with(source, &config),
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
            "local function e(e)local t if e then t=g()print(t)end if e then t=h()print(t)end end return e"
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
            "local function e()local e,t e=1 do t=2 print(t)end return e end return e"
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
            "function W.new(t)local e for n=1,3 do if t then e=f()print(e)end end while t do \
             e=g()print(e)end end"
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
    }

    #[test]
    fn hoisted_declaration_leaves_registers_for_a_wide_statement() {
        let body = (0..190)
            .map(|i| format!("do local v{i} = g() print(v{i}) end\n"))
            .collect::<Vec<_>>()
            .concat();
        let args = (0..100).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
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
            name_count + 101 <= MAX_REGISTERS,
            "{name_count} hoisted locals leave no room for the call's 101 registers"
        );
    }

    #[test]
    fn hoisted_declaration_stays_within_the_local_limit() {
        let body = (0..205)
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
            let result = apply(&source);
            assert!(result.contains(lifted), "{source}\n{result}");
        }
    }

    #[test]
    fn a_read_in_a_type_captures_nothing() {
        assert_outputs(
            apply,
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

    /// A closure in a type never runs either, so nothing it reads is
    /// captured.
    #[test]
    fn a_function_in_a_type_lifts_what_its_closures_read() {
        let result = apply(
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
            "local function f()local x,w do x=1 print(x::typeof(function()local n,m,o do n=1 \
             h=function()return n end end do m=2 print(m)end do o=3 print(o)end end))end do w=2 \
             print(w)end end"
        );
    }

    #[test]
    fn a_type_function_reads_like_a_closure() {
        // `x` in the type function is a global the hoisted `x` would capture.
        let result = apply(
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
