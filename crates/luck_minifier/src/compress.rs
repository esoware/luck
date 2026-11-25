//! The compressor: passes of an in-place walk that runs every rewrite the
//! tree allows as it leaves each node, until a pass changes nothing, as
//! `oxc_minifier`'s peephole loop runs.
//!
//! A rewrite reads each name's symbol and references from `luck_semantic`'s
//! [`Scoping`], by the name's index. One that drops part of the tree marks
//! the references in it as dropped, which every query of a symbol's
//! references skips, so each decision reads the tree as it stands. At the
//! end of a pass the symbols' lists lose the dropped references. Debug
//! builds check that the lists match a fresh analysis once the passes
//! settle.
//!
//! Once they settle, `lift_locals` hoists each function's declarations,
//! which reshapes its bindings, so the scoping is built again and the passes
//! run again on what lifting left.
//!
//! Each rewrite lives in the module named after the flag that turns it on.
//! Rewrites build their nodes in the shape the parser gives the text they
//! print, so the fixpoint's own output is a fixpoint. The printer writes
//! whatever parentheses grouping needs, so a rewrite adds only those that
//! change what they hold. A literal is a value, which `luck_codegen` spells
//! the shortest way, so no rewrite respells one.

mod fold_constants;
mod inline_locals;
mod lift_locals;
mod merge_locals;
mod remove_dead_code;
mod simplify_indexes;
mod simplify_parens;
mod simplify_statements;

use std::mem;

use luck_allocator::{Allocator, List};
use luck_ast::{
    Assignment, BinaryOp, Block, ConditionalBinding, Expression, Field, FunctionArgs, FunctionBody,
    GenericFor, Name, ReturnStatement, Statement, Suffix, SuffixedExpression, TableConstructor,
    TreeCounts, Type, TypedName, Visit, VisitMut,
};
use luck_data_structures::BitSet;
use luck_semantic::{Scoping, SymbolId};
use luck_value::ConstantEvaluation as _;

use self::inline_locals::Held;
use self::remove_dead_code::{UnusedLocalAction, unused_local_action};
use self::simplify_parens::Place;
use crate::binding::{NameIndexes, ResolvedNames, analyze};
use crate::limits::{NUMERIC_FOR_REGISTERS, generic_for_registers, parameter_locals};
use crate::transform_config::TransformConfig;

/// A pass leaves another only for what its own rewrites exposed, so a
/// handful settle any input, and the caps bound only a rewrite that never
/// stops, which debug builds report.
const MAX_PASSES: usize = 10;
const MAX_ROUNDS: usize = 4;

/// Compresses `chunk` until a pass changes nothing, then lifts its
/// functions' declarations and compresses what that leaves, until lifting
/// finds nothing more. A lift indexes the names it hoists from
/// `name_indexes`.
///
/// Returns the last scoping, whose tables a later analysis can reuse.
pub(crate) fn compress<'ast>(
    allocator: &'ast Allocator,
    chunk: &mut Block<'ast>,
    counts: TreeCounts,
    config: TransformConfig,
    name_indexes: &mut NameIndexes,
) -> Scoping {
    let mut scoping = analyze(chunk, counts, None);
    for round in 1..=MAX_ROUNDS {
        let mut compressor = Compressor::new(allocator, config, &mut scoping);
        compressor.run(chunk);
        #[cfg(debug_assertions)]
        compressor.assert_references_match(chunk, counts);
        if !(config.lift_locals && compressor.lift(chunk, name_indexes)) {
            break;
        }
        debug_assert!(round < MAX_ROUNDS, "lifting a lifted tree lifted more");
        scoping = analyze(chunk, counts, Some(scoping));
    }
    scoping
}

/// Drops the statements at `indexes`, which ascend, and keeps the others in
/// order, in place.
fn remove_stmts(stmts: &mut List<'_, Statement<'_>>, indexes: &[usize]) {
    if indexes.is_empty() {
        return;
    }
    let mut removed = indexes.iter().peekable();
    let mut kept = 0;
    for index in 0..stmts.len() {
        if removed.next_if_eq(&&index).is_none() {
            stmts.swap(kept, index);
            kept += 1;
        }
    }
    stmts.truncate(kept);
}

/// Marks the references in a part of the tree a rewrite drops.
struct Dropper<'walk> {
    scoping: &'walk Scoping,
    dropped: &'walk mut BitSet,
}

impl<'tree> Visit<'tree> for Dropper<'_> {
    fn visit_name(&mut self, name: &'tree Name<'tree>) {
        if let Some(reference) = self.scoping.reference_of(name) {
            self.dropped.insert(reference.index());
        }
    }
}

pub(crate) struct Compressor<'walk, 'ast> {
    allocator: &'ast Allocator,
    config: TransformConfig,
    scoping: &'walk mut Scoping,
    /// The references whose names a rewrite dropped since the scoping was
    /// built.
    dropped: BitSet,
    has_changed: bool,
    /// Levels of nesting open where the walk is.
    depth: usize,
    /// Types the walk is in, where it keeps every truncation.
    type_depth: usize,
    /// The initializers of the locals being inlined, each until the block
    /// that declares it ends, innermost block's last.
    held: Vec<Held<'ast>>,
    /// Each symbol's place in `held`, or [`inline_locals::NOT_HELD`].
    held_index: Vec<u32>,
    /// How many of `held` wait for their read.
    waiting: usize,
    /// Whether a value has landed as a run among the operands of the run
    /// being walked, which then joins it where it groups the same, so a
    /// chain of inlined values nests no deeper than one.
    has_landed_run: bool,
    /// The inlined locals taken in by the values landed since the
    /// initializer being walked began.
    landed_locals: usize,
    /// Each initializer's `landed_locals`, for the `local`s being walked,
    /// innermost last.
    landed_counts: Vec<usize>,
    live_registers: usize,
    /// The bindings of the group being merged, or of the bare `local` being
    /// fused.
    group: Vec<SymbolId>,
    /// The calls the unused `local`s of the block the walk is leaving
    /// leave, each after its index.
    late_calls: Vec<(usize, Statement<'ast>)>,
    /// The indexes of the statements a block loses.
    removed_stmts: Vec<usize>,
    /// A block's statements while `do` blocks in it give up theirs.
    unwrapped_stmts: Vec<Statement<'ast>>,
    /// Whether the `if` the walk enters next is the one statement of an `if`
    /// that merges the run of nested `if`s it starts.
    is_in_if_run: bool,
}

impl<'walk, 'ast> Compressor<'walk, 'ast> {
    pub(crate) fn new(
        allocator: &'ast Allocator,
        config: TransformConfig,
        scoping: &'walk mut Scoping,
    ) -> Self {
        let dropped = BitSet::new(scoping.references().len());
        let held_index = vec![inline_locals::NOT_HELD; scoping.symbols().len()];
        Self {
            allocator,
            config,
            scoping,
            dropped,
            has_changed: false,
            depth: 0,
            type_depth: 0,
            held: Vec::new(),
            held_index,
            waiting: 0,
            has_landed_run: false,
            landed_locals: 0,
            landed_counts: Vec::new(),
            live_registers: 0,
            group: Vec::new(),
            late_calls: Vec::new(),
            removed_stmts: Vec::new(),
            unwrapped_stmts: Vec::new(),
            is_in_if_run: false,
        }
    }

    /// Walks `block` until a pass changes nothing.
    fn run(&mut self, block: &mut Block<'ast>) {
        for pass in 1..=MAX_PASSES {
            if !self.pass(block) {
                return;
            }
            debug_assert!(pass < MAX_PASSES, "a pass still changed the tree after {pass} passes");
        }
    }

    /// One walk over `block`, after which the symbols' lists lose the
    /// references it dropped, which says whether it changed anything.
    pub(crate) fn pass(&mut self, block: &mut Block<'ast>) -> bool {
        self.has_changed = false;
        self.visit_block(block);
        // The chunk is a function too, and returns nothing either way.
        if self.config.remove_dead_code {
            self.has_changed |= remove_dead_code::drop_bare_return(block);
        }
        if self.has_changed {
            let dropped = &self.dropped;
            self.scoping
                .retain_resolved_references(|reference| !dropped.contains(reference.index()));
        }
        self.has_changed
    }

    /// Checks the symbols' lists against a fresh analysis of the tree: each
    /// name that refers to a symbol is live and in its list, and no list
    /// holds a reference whose name left the tree.
    #[cfg(debug_assertions)]
    fn assert_references_match(&self, chunk: &Block<'_>, counts: TreeCounts) {
        let fresh = analyze(chunk, counts, None);
        let mut check = FreshCheck {
            scoping: self.scoping,
            dropped: &self.dropped,
            fresh: &fresh,
            pairs: vec![None; self.scoping.symbols().len()],
        };
        check.visit_block(chunk);
        for (symbol, pair) in self.scoping.symbols().iter().zip(&check.pairs) {
            let kept = self.scoping.resolved_references(symbol.id).len();
            let found =
                pair.map_or(0, |fresh_symbol| fresh.resolved_references(fresh_symbol).len());
            assert_eq!(kept, found, "a rewrite dropped a name without marking its reference");
        }
    }

    const fn dropper(&mut self) -> Dropper<'_> {
        Dropper { scoping: self.scoping, dropped: &mut self.dropped }
    }

    /// Marks the references in `expr`, which the walk drops.
    fn drop_expr(&mut self, expr: &Expression<'ast>) {
        self.dropper().visit_expr(expr);
    }

    fn drop_stmt(&mut self, stmt: &Statement<'ast>) {
        self.dropper().visit_stmt(stmt);
    }

    fn drop_block(&mut self, block: &Block<'ast>) {
        self.dropper().visit_block(block);
    }

    /// Walks the annotation of a condition's binding, if there is one, which
    /// the walk reaches before the condition, as the source has them.
    fn visit_binding(&mut self, binding: Option<&mut ConditionalBinding<'ast>>) {
        if let Some(ty) = binding.and_then(|binding| binding.name.type_annotation.as_deref_mut()) {
            self.visit_type(ty);
        }
    }

    /// Walks a block, where a binding of the condition that guards it, if
    /// one does, takes a register.
    fn visit_bound_block(&mut self, is_bound: bool, block: &mut Block<'ast>) {
        let registers = usize::from(is_bound);
        self.live_registers += registers;
        self.visit_block(block);
        self.live_registers -= registers;
    }

    /// Marks the references in the annotation of a name the walk drops from
    /// its `local`.
    fn drop_annotation(&mut self, local_name: &TypedName<'ast>) {
        if let Some(ty) = &local_name.type_annotation {
            self.dropper().visit_type(ty);
        }
    }

    /// Whether code the tree keeps reads or writes the symbol `name`
    /// declares. A name that declares none is taken to be used, and a
    /// recursive `local function`'s references to itself in its body keep
    /// nothing alive.
    fn is_used(&self, name: &Name<'_>) -> bool {
        self.scoping.symbol_of(name).is_none_or(|symbol| {
            self.scoping.resolved_references(symbol).iter().any(|&reference| {
                !self.dropped.contains(reference.index())
                    && !self.scoping.reference(reference).is_recursive
            })
        })
    }

    /// Walks each statement, and drops the ones the walk leaves nothing of.
    fn visit_stmts(&mut self, stmts: &mut List<'ast, Statement<'ast>>) {
        let mut kept = 0;
        for index in 0..stmts.len() {
            if self.visit_kept_stmt(&mut stmts[index]) {
                stmts.swap(kept, index);
                self.live_registers += merge_locals::declared_count(&stmts[kept]);
                kept += 1;
            } else {
                self.has_changed = true;
            }
        }
        stmts.truncate(kept);
    }

    /// Walks `stmt` and rewrites it, and says whether anything of it stays.
    fn visit_kept_stmt(&mut self, stmt: &mut Statement<'ast>) -> bool {
        let should_remove_dead_code = self.config.remove_dead_code;
        // A local nothing references when the walk reaches it goes unwalked.
        match stmt {
            Statement::LocalAssignment(local) => {
                if should_remove_dead_code
                    && local.names.iter().all(|local_name| !self.is_used(&local_name.name))
                    && unused_local_action(local, self.names()) == UnusedLocalAction::Remove
                {
                    self.drop_stmt(stmt);
                    return false;
                }
                return self.visit_local(stmt);
            }
            Statement::LocalFunction(local_function) => {
                if should_remove_dead_code
                    && !local_function.kind.is_exported()
                    && !self.is_used(&local_function.name)
                {
                    self.drop_stmt(stmt);
                    return false;
                }
            }
            Statement::IfStatement(_) => return self.visit_if(stmt),
            Statement::WhileLoop(while_loop) => {
                self.visit_binding(while_loop.binding.as_deref_mut());
                self.visit_expr(&mut while_loop.condition);
                if should_remove_dead_code
                    && while_loop.condition.get_side_free_boolean_value(&self.names())
                        == Some(false)
                {
                    self.drop_stmt(stmt);
                    return false;
                }
                self.visit_bound_block(while_loop.binding.is_some(), &mut while_loop.block);
                return true;
            }
            Statement::RepeatLoop(_) => return self.visit_repeat(stmt),
            Statement::DoBlock(do_block) => {
                self.visit_block(&mut do_block.block);
                return !(should_remove_dead_code && do_block.block.stmts.is_empty());
            }
            Statement::NumericFor(numeric_for) => {
                if should_remove_dead_code
                    && remove_dead_code::has_default_step(numeric_for, self.names())
                    && let Some(step) = numeric_for.step.take()
                {
                    self.drop_expr(&step);
                    self.has_changed = true;
                }
                let outer_registers = self.live_registers;
                self.live_registers += NUMERIC_FOR_REGISTERS;
                self.walk_stmt(stmt);
                self.live_registers = outer_registers;
                return true;
            }
            Statement::GenericFor(generic_for) => {
                let outer_registers = self.live_registers;
                self.live_registers += generic_for_registers(generic_for.names.len());
                self.walk_stmt(stmt);
                self.live_registers = outer_registers;
                return true;
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
        self.walk_stmt(stmt);
        true
    }

    /// What a block takes once its statements have theirs: the locals it
    /// inlined go, then its unused locals, then `do` blocks give up their
    /// statements, neighbors merge, and a run of `if ... return` folds.
    fn finish_block(&mut self, block: &mut Block<'ast>, outer_registers: usize, held_start: usize) {
        self.settle_held(&mut block.stmts, held_start);
        if self.config.remove_dead_code {
            self.drop_unused(&mut block.stmts);
        }
        if self.config.simplify_statements {
            self.unwrap_do_blocks(block);
        }
        if self.config.merge_locals {
            self.live_registers = outer_registers;
            self.merge_stmts(&mut block.stmts);
        }
        if self.config.simplify_statements {
            self.convert_if_return_chain(block);
        }
        self.live_registers = outer_registers;
    }

    /// Where an expression stands that takes only its first value, or in a
    /// type, where the walk keeps every truncation.
    const fn one_value(&self) -> Place {
        if self.type_depth > 0 { Place::AllValues } else { Place::OneValue }
    }

    /// Where the item at `index` of a list of `count` stands: the last may
    /// spread its values when `can_spread`.
    const fn list_item(&self, index: usize, count: usize, can_spread: bool) -> Place {
        if can_spread && index + 1 == count { Place::AllValues } else { self.one_value() }
    }

    /// Walks the values of a list that fills `targets` slots.
    fn visit_values(&mut self, values: &mut [Expression<'ast>], targets: usize) {
        let count = values.len();
        for (index, value) in values.iter_mut().enumerate() {
            let place = self.list_item(index, count, targets > count);
            self.visit_expr_in(value, place);
        }
    }

    /// Walks an expression and rewrites it at `place`.
    fn visit_expr_in(&mut self, expr: &mut Expression<'ast>, place: Place) {
        match expr {
            Expression::Name(name) => {
                if self.waiting > 0
                    && let Some(value) = self.take_held(name)
                {
                    *expr = value;
                }
                return;
            }
            Expression::BinaryOp(run) => self.walk_run(run),
            Expression::UnaryOp(unary_op) => {
                if self.descend() {
                    self.visit_operand(&mut unary_op.operand, Place::OneValue);
                }
                self.ascend();
            }
            Expression::TypeCast(cast) => {
                if self.descend() {
                    self.visit_operand(&mut cast.expr, Place::OneValue);
                    self.visit_type(&mut cast.ty);
                }
                self.ascend();
            }
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_) => self.walk_expr(expr),
        }
        self.leave_expr(expr, place);
    }

    /// Its own frame, so the locals it needs stay out of the one every level
    /// of nesting takes.
    fn walk_run(&mut self, run: &mut BinaryOp<'ast>) {
        if self.descend() {
            let outer_has_landed_run = mem::replace(&mut self.has_landed_run, false);
            self.visit_operand(&mut run.first, Place::OneValue);
            for link in &mut *run.links {
                self.visit_operand(&mut link.operand, Place::OneValue);
            }
            if mem::replace(&mut self.has_landed_run, outer_has_landed_run) {
                run.splice_operands(self.allocator);
            }
        }
        self.ascend();
    }

    /// The rewrites an expression at `place` takes once its operands have
    /// theirs.
    fn leave_expr(&mut self, expr: &mut Expression<'ast>, place: Place) {
        let allocator = self.allocator;
        match expr {
            Expression::BinaryOp(_) => {
                if self.config.remove_dead_code && self.fold_redundant_ternary(expr) {
                    self.has_changed = true;
                    return self.leave_expr(expr, place);
                }
                if self.config.fold_constants && self.fold(expr) {
                    self.has_changed = true;
                    return self.leave_expr(expr, place);
                }
                // A run a rewrite built has operands no walk left, and an
                // operand takes one value.
                if self.config.simplify_parens
                    && let Expression::BinaryOp(run) = expr
                {
                    let mut has_stripped = simplify_parens::strip(&mut run.first, Place::OneValue);
                    for link in &mut *run.links {
                        has_stripped |= simplify_parens::strip(&mut link.operand, Place::OneValue);
                    }
                    if has_stripped | run.splice_operands(allocator) {
                        self.has_changed = true;
                    }
                }
            }
            Expression::UnaryOp(_) => {
                if self.config.fold_constants && self.fold(expr) {
                    self.has_changed = true;
                    return self.leave_expr(expr, place);
                }
                // So does an operator a rewrite built.
                if self.config.simplify_parens
                    && let Expression::UnaryOp(unary_op) = expr
                    && simplify_parens::strip(&mut unary_op.operand, Place::OneValue)
                {
                    self.has_changed = true;
                }
            }
            Expression::Parenthesized(_) => {
                if self.config.simplify_parens && simplify_parens::strip(expr, place) {
                    self.has_changed = true;
                }
            }
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => {}
        }
    }

    /// The names as the scoping resolves them, for what `luck_value` tells of
    /// an expression.
    const fn names(&self) -> ResolvedNames<'_> {
        ResolvedNames(self.scoping)
    }
}

impl<'ast> VisitMut<'ast> for Compressor<'_, 'ast> {
    fn descend(&mut self) -> bool {
        self.depth += 1;
        true
    }

    fn ascend(&mut self) {
        self.depth -= 1;
    }

    fn visit_block(&mut self, block: &mut Block<'ast>) {
        self.descend();
        let outer_registers = self.live_registers;
        let held_start = self.held.len();
        self.visit_stmts(&mut block.stmts);
        self.finish_block(block, outer_registers, held_start);
        self.ascend();
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        let place = self.one_value();
        self.visit_expr_in(expr, place);
    }

    /// A name the walk reaches outside an expression, as `function t.f()`
    /// reads `t`, can't take a value, so the local it reads keeps its own.
    fn visit_name(&mut self, name: &mut Name<'ast>) {
        if self.waiting > 0 {
            self.refuse_held(name);
        }
    }

    /// A prefix's parentheses go with the chain's, since an instantiation
    /// passes every value of what it follows.
    fn visit_suffixed_expr(&mut self, suffixed_expr: &mut SuffixedExpression<'ast>) {
        if self.descend() {
            self.visit_expr_in(&mut suffixed_expr.prefix, Place::AllValues);
            for suffix in &mut *suffixed_expr.suffixes {
                match suffix {
                    Suffix::Field(_) => {}
                    Suffix::Index { index, .. } => {
                        self.visit_expr(index);
                        if self.config.simplify_indexes && simplify_indexes::index_to_field(suffix)
                        {
                            self.has_changed = true;
                        }
                    }
                    Suffix::Call(args) => self.visit_function_args(args),
                    Suffix::MethodCall(call) => {
                        if let Some(type_args) = &mut call.type_args {
                            self.visit_type_args(type_args);
                        }
                        self.visit_function_args(&mut call.args);
                    }
                    Suffix::TypeInstantiation { type_args, .. } => self.visit_type_args(type_args),
                }
            }
            if self.config.simplify_parens
                && simplify_parens::strip_prefix(self.allocator, suffixed_expr)
            {
                self.has_changed = true;
            }
        }
        self.ascend();
    }

    fn visit_function_args(&mut self, function_args: &mut FunctionArgs<'ast>) {
        if let FunctionArgs::Parenthesized { args, .. } = function_args {
            self.visit_values(args, usize::MAX);
        } else {
            self.walk_function_args(function_args);
        }
        if self.config.simplify_parens && simplify_parens::strip_call_args(function_args) {
            self.has_changed = true;
        }
    }

    /// The last field spreads its values when it's positional.
    fn visit_table_constructor(&mut self, table_constructor: &mut TableConstructor<'ast>) {
        if self.descend() {
            let count = table_constructor.fields.len();
            for (index, field) in table_constructor.fields.iter_mut().enumerate() {
                if let Field::Positional(value) = field {
                    let place = self.list_item(index, count, true);
                    self.visit_expr_in(value, place);
                } else {
                    self.visit_field(field);
                }
            }
        }
        self.ascend();
        if self.config.simplify_indexes && simplify_indexes::simplify_table(table_constructor) {
            self.has_changed = true;
        }
    }

    fn visit_return_stmt(&mut self, return_stmt: &mut ReturnStatement<'ast>) {
        self.visit_values(&mut return_stmt.exprs, usize::MAX);
    }

    fn visit_assignment(&mut self, assignment: &mut Assignment<'ast>) {
        for target in &mut assignment.targets {
            self.visit_var(target);
        }
        let targets = assignment.targets.len();
        self.visit_values(&mut assignment.values, targets);
    }

    /// The values a generic `for` takes are its iterator, state, and control
    /// value, so the last may spread.
    fn visit_generic_for(&mut self, generic_for: &mut GenericFor<'ast>) {
        for param in &mut generic_for.names {
            self.visit_typed_name(param);
        }
        self.visit_values(&mut generic_for.exprs, usize::MAX);
        self.visit_block(&mut generic_for.block);
    }

    fn visit_function_body(&mut self, function_body: &mut FunctionBody<'ast>) {
        let outer_registers =
            mem::replace(&mut self.live_registers, parameter_locals(function_body));
        self.walk_function_body(function_body);
        if self.config.remove_dead_code {
            self.has_changed |= remove_dead_code::drop_bare_return(&mut function_body.block);
        }
        self.live_registers = outer_registers;
    }

    fn visit_type(&mut self, ty: &mut Type<'ast>) {
        self.type_depth += 1;
        self.walk_type(ty);
        self.type_depth -= 1;
    }
}

/// Pairs each symbol the scoping has with the one a fresh analysis finds
/// for the same name, and checks that every name that refers to one refers
/// to its pair, by a live reference.
#[cfg(debug_assertions)]
struct FreshCheck<'check> {
    scoping: &'check Scoping,
    dropped: &'check BitSet,
    fresh: &'check Scoping,
    pairs: Vec<Option<SymbolId>>,
}

#[cfg(debug_assertions)]
impl<'tree> Visit<'tree> for FreshCheck<'_> {
    fn visit_name(&mut self, name: &'tree Name<'tree>) {
        if let (Some(symbol), Some(fresh)) =
            (self.scoping.symbol_of(name), self.fresh.symbol_of(name))
        {
            self.pairs[symbol.index()] = Some(fresh);
        }
        let kept = self.scoping.reference_of(name);
        let found = self.fresh.reference_of(name);
        assert_eq!(kept.is_some(), found.is_some(), "{name:?} refers in one analysis only");
        if let (Some(reference), Some(fresh)) = (kept, found) {
            assert!(!self.dropped.contains(reference.index()), "{name:?} is marked dropped");
            let fresh_resolved = self.fresh.reference(fresh).resolved;
            let paired = self.scoping.reference(reference).resolved.and_then(|symbol| {
                let pair = &mut self.pairs[symbol.index()];
                // An implicit `self` has no name, so its first reference
                // pairs it.
                if pair.is_none() {
                    *pair = fresh_resolved;
                }
                *pair
            });
            assert_eq!(paired, fresh_resolved, "{name:?} resolves elsewhere");
        }
    }
}
