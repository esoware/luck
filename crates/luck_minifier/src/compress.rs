//! The compressor: an in-place walk that runs every rewrite the tree allows
//! as it leaves each node.
//!
//! The walk reads each name's binding from the name itself and decides by
//! what the bind walk counted of the binding, which every rewrite keeps true
//! by uncounting what it drops. A block's locals are only ever referenced in
//! the block, so the walk drops the ones nothing references as it leaves it,
//! and one walk reaches the fixpoint. It walks again only when it leaves a
//! rewrite behind that it could see but not make, after a fresh count.
//!
//! Each rewrite lives in the module named after the flag that turns it on,
//! and `respell` holds what two flags share. Rewrites build their nodes in
//! the shape the parser gives the text they print, which is what makes the
//! fixpoint's own output a fixpoint.

mod fold_constants;
mod inline_locals;
mod lift_locals;
mod merge_locals;
mod remove_dead_code;
mod respell;
mod shorten_numbers;
mod shorten_strings;
mod simplify_indexes;
mod simplify_parens;
mod simplify_statements;

use core::cell::Cell;
use core::mem;

use luck_allocator::{Allocator, List};
use luck_ast::{
    AttributedName, BinaryOp, Block, Expression, FunctionArgs, FunctionBody, Statement, Suffix,
    SuffixedExpression, TableConstructor, Type, Visit as _, VisitMut, is_prefix_expr,
};
use luck_token::{LuaVersion, NumberSubtypes};

use self::inline_locals::{OperandPosition, Value};
use self::lift_locals::Lifter;
use self::remove_dead_code::{UnusedLocalAction, unused_local_action};
use self::respell::Respeller;
use crate::binding::{Bindings, Counter, NONE, is_droppable_function, local_of};
use crate::expr::{constant_truthiness, fits_at_depth, inner_operand_side, parenthesize};
use crate::limits::{GENERIC_FOR_STATE_LOCALS, NUMERIC_FOR_STATE_LOCALS, parameter_locals};
use crate::transform_config::TransformConfig;

/// A walk leaves another behind only for a rewrite it exposed late, so a
/// handful settle any input, and the cap only bounds one that never stops,
/// which the fixpoint tests catch.
const MAX_WALKS: usize = 8;

/// Compresses `block`, whose bindings `bindings` holds, until a walk leaves
/// nothing for another.
pub(crate) fn compress<'ast>(
    allocator: &'ast Allocator,
    version: LuaVersion,
    config: TransformConfig,
    bindings: &mut Bindings,
    block: &mut Block<'ast>,
) {
    let mut compressor = Compressor::new(allocator, version, config, bindings);
    for walk in 1..=MAX_WALKS {
        compressor.walk(block);
        if !compressor.needs_revisit || walk == MAX_WALKS {
            break;
        }
        compressor.recount(block);
    }
}

#[expect(clippy::struct_excessive_bools, reason = "each bool is a separate fact about the walk")]
pub(crate) struct Compressor<'walk, 'ast> {
    allocator: &'ast Allocator,
    version: LuaVersion,
    subtypes: NumberSubtypes,
    config: TransformConfig,
    bindings: &'walk mut Bindings,
    has_changed: bool,
    /// Whether the walk left a rewrite it exposed but couldn't make.
    needs_revisit: bool,
    /// Levels of nesting open where the walk is.
    depth: usize,
    /// Functions open where the walk is, and one past that count at the
    /// innermost type the walk is in, or 0, as the bind walk counts them.
    function_depth: u32,
    type_floor: u32,
    /// The inlined locals' values waiting for their one read, each indexed
    /// by its binding's `held_value`.
    values: Vec<Option<Value<'ast>>>,
    held_count: usize,
    /// Whether a rewrite stayed undone for want of depth, which it may have
    /// once the tree around it flattens.
    has_depth_refusal: Cell<bool>,
    /// Whether a value has landed bare as an operand since the run being
    /// walked began, so the run may hold one to splice.
    has_bare_operand: bool,
    /// The inlined locals taken in by the values landed since the
    /// initializer being walked began.
    landed_locals: usize,
    /// Each initializer's `landed_locals`, for the `local`s being walked,
    /// innermost last.
    landed_counts: Vec<usize>,
    live_registers: usize,
    /// The bindings of the group being merged.
    group: Vec<usize>,
    /// The calls the unused `local`s of the block the walk is leaving
    /// leave, each after its index.
    late_calls: Vec<(usize, Statement<'ast>)>,
    /// A block's statements while `do` blocks in it give up theirs.
    unwrapped_stmts: Vec<Statement<'ast>>,
    /// Whether the `if` the walk enters next is the one statement of an `if`
    /// that merges the run of nested `if`s it starts.
    is_in_if_run: bool,
    /// Whether the `local` the walk enters next holds a late inlined read,
    /// so the names it declares are read only where the walk has been.
    is_late_read: bool,
    respeller: Respeller,
    lifter: Lifter<'ast>,
}

impl<'walk, 'ast> Compressor<'walk, 'ast> {
    pub(crate) fn new(
        allocator: &'ast Allocator,
        version: LuaVersion,
        config: TransformConfig,
        bindings: &'walk mut Bindings,
    ) -> Self {
        Self {
            allocator,
            version,
            subtypes: NumberSubtypes::from(version),
            config,
            bindings,
            has_changed: false,
            needs_revisit: false,
            depth: 0,
            function_depth: 0,
            type_floor: 0,
            values: Vec::new(),
            held_count: 0,
            has_depth_refusal: Cell::new(false),
            has_bare_operand: false,
            landed_locals: 0,
            landed_counts: Vec::new(),
            live_registers: 0,
            group: Vec::new(),
            late_calls: Vec::new(),
            unwrapped_stmts: Vec::new(),
            is_in_if_run: false,
            is_late_read: false,
            respeller: Respeller::default(),
            lifter: Lifter::default(),
        }
    }

    /// One walk over `block`, which says whether it changed anything.
    pub(crate) fn walk(&mut self, block: &mut Block<'ast>) -> bool {
        self.has_changed = false;
        self.needs_revisit = false;
        self.has_depth_refusal.set(false);
        self.depth = 0;
        self.function_depth = 0;
        self.type_floor = 0;
        self.values.clear();
        self.held_count = 0;
        self.live_registers = 0;
        self.visit_block(block);
        // The chunk is a function too, and returns nothing either way.
        if self.config.remove_dead_code {
            self.has_changed |= remove_dead_code::drop_bare_return(block);
        }
        self.needs_revisit |= self.has_depth_refusal.get() && self.has_changed;
        self.needs_revisit |= self.take_freed_names();
        self.has_changed
    }

    /// Whether `expr` still nests within the parser's cap once it sits
    /// `added_levels` below `depth`, noting when it doesn't.
    fn fits(&self, depth: usize, added_levels: usize, expr: &Expression<'_>) -> bool {
        let fits = fits_at_depth(depth, added_levels, expr);
        if !fits {
            self.has_depth_refusal.set(true);
        }
        fits
    }

    /// Counts every binding's references afresh, for a walk after one that
    /// changed where they sit.
    fn recount(&mut self, block: &Block<'ast>) {
        for binding in &mut self.bindings.entries {
            binding.reads = 0;
            binding.writes = 0;
            binding.captures = 0;
            binding.held_value = NONE;
        }
        self.bindings.global_uses.fill(0);
        self.bindings.name_captures.fill(0);
        self.bindings.global_declarations.fill(0);
        let mut counter = Counter {
            bindings: self.bindings,
            is_retracting: false,
            function_depth: 0,
            type_floor: 0,
            depth: 0,
        };
        counter.visit_block(block);
    }

    const fn counter(&mut self) -> Counter<'_> {
        Counter {
            bindings: self.bindings,
            is_retracting: true,
            function_depth: self.function_depth,
            type_floor: self.type_floor,
            depth: 0,
        }
    }

    /// Uncounts the references in `expr`, which the walk drops.
    fn retract_expr(&mut self, expr: &Expression<'ast>) {
        self.counter().visit_expr(expr);
    }

    fn retract_stmt(&mut self, stmt: &Statement<'ast>) {
        self.counter().visit_stmt(stmt);
    }

    fn retract_block(&mut self, block: &Block<'ast>) {
        self.counter().visit_block(block);
    }

    /// Uncounts the references in the annotation of a name the walk drops
    /// from its `local`.
    fn retract_annotation(&mut self, attributed: &AttributedName<'ast>) {
        if let Some(ty) = &attributed.type_annotation {
            self.counter().visit_type(ty);
        }
    }

    /// Walks each statement, and drops the ones the walk leaves nothing of.
    fn visit_stmts(&mut self, stmts: &mut List<'ast, Statement<'ast>>) {
        let mut kept = 0usize;
        for index in 0..stmts.len() {
            let Some(stmt) = stmts.get_mut(index) else {
                break;
            };
            if self.visit_kept_stmt(stmt) {
                stmts.swap(kept, index);
                if let Some(kept_stmt) = stmts.get(kept) {
                    self.live_registers =
                        self.live_registers.saturating_add(merge_locals::declared_count(kept_stmt));
                }
                kept = kept.saturating_add(1);
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
                    && local.names.iter().all(|attributed| !self.is_used(&attributed.name))
                    && unused_local_action(local, self.version) == UnusedLocalAction::Remove
                {
                    self.retract_stmt(stmt);
                    return false;
                }
                return self.visit_local(stmt);
            }
            Statement::LocalFunction(local_function) => {
                if should_remove_dead_code
                    && is_droppable_function(local_function, self.version)
                    && !self.is_used(&local_function.name)
                {
                    self.retract_stmt(stmt);
                    return false;
                }
                let id = local_of(&local_function.name);
                self.set_open(id, true);
                self.walk_stmt(stmt);
                self.set_open(id, false);
                return true;
            }
            Statement::IfStatement(_) => return self.visit_if(stmt),
            Statement::WhileLoop(while_loop) => {
                self.visit_expr(&mut while_loop.condition);
                if should_remove_dead_code
                    && constant_truthiness(&while_loop.condition) == Some(false)
                {
                    self.retract_block(&while_loop.block);
                    return false;
                }
                self.visit_block(&mut while_loop.block);
                return true;
            }
            Statement::RepeatLoop(_) => return self.visit_repeat(stmt),
            Statement::DoBlock(do_block) => {
                self.visit_block(&mut do_block.block);
                return !(should_remove_dead_code
                    && remove_dead_code::is_empty_block(&do_block.block));
            }
            Statement::NumericFor(numeric_for) => {
                if should_remove_dead_code && remove_dead_code::has_default_step(numeric_for) {
                    numeric_for.step = None;
                    self.has_changed = true;
                }
                let outer_registers = self.live_registers;
                self.live_registers =
                    self.live_registers.saturating_add(NUMERIC_FOR_STATE_LOCALS.saturating_add(1));
                self.walk_stmt(stmt);
                self.live_registers = outer_registers;
                return true;
            }
            Statement::GenericFor(generic_for) => {
                let outer_registers = self.live_registers;
                let loop_registers =
                    GENERIC_FOR_STATE_LOCALS.saturating_add(generic_for.names.len());
                self.live_registers = self.live_registers.saturating_add(loop_registers);
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
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
        true
    }

    /// A `local function`'s references to itself in its own body keep
    /// nothing alive.
    fn set_open(&mut self, id: Option<usize>, is_open: bool) {
        if let Some(binding) = id.and_then(|own| self.bindings.get_mut(own)) {
            binding.is_open = is_open;
        }
    }

    /// What a block takes once its statements have theirs: its unused
    /// locals go, then `do` blocks give up their statements, neighbors
    /// merge, and a run of `if ... return` folds.
    fn finish_block(&mut self, block: &mut Block<'ast>, outer_registers: usize) {
        if self.config.remove_dead_code {
            self.drop_unused(&mut block.stmts);
        }
        if self.config.inline_locals {
            self.inline_late(block, outer_registers);
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

    /// Walks a run's operands, landing an inlined value bare wherever it
    /// keeps its grouping, and splices a run that lands bare at the run's
    /// own level.
    fn walk_run(&mut self, run: &mut BinaryOp<'ast>) {
        if self.descend() {
            if self.held_count > 0 {
                self.close_casts_before_less_than(run);
            }
            let op = run.links.first().op;
            let inner_position = OperandPosition::of_side(op, inner_operand_side(run));
            let outer_has_bare_operand = mem::replace(&mut self.has_bare_operand, false);
            self.visit_operand(&mut run.first, OperandPosition::Left(op));
            let last_index = run.links.len().saturating_sub(1);
            for (i, link) in run.links.iter_mut().enumerate() {
                let position =
                    if i == last_index { OperandPosition::Right(op) } else { inner_position };
                self.visit_operand(&mut link.operand, position);
            }
            if mem::replace(&mut self.has_bare_operand, outer_has_bare_operand) {
                run.splice_operands(self.allocator);
            }
        }
        self.ascend();
    }

    /// Walks an expression and rewrites it, as an operand of an operator or
    /// a cast, or standing alone, where it groups with nothing around it.
    fn visit_expr_in(&mut self, expr: &mut Expression<'ast>, is_operand: bool) {
        match expr {
            Expression::Name(name) => {
                if self.held_count > 0
                    && let Some(value) = local_of(name).and_then(|id| self.take_value(id))
                {
                    *expr = value;
                }
                return;
            }
            Expression::BinaryOp(run) => self.walk_run(run),
            Expression::UnaryOp(unary_op) => {
                if self.descend() {
                    let position = OperandPosition::Unary(unary_op.op);
                    self.visit_operand(&mut unary_op.operand, position);
                }
                self.ascend();
            }
            // Luau
            Expression::TypeCast(cast) => {
                if self.descend() {
                    self.visit_operand(&mut cast.expr, OperandPosition::Cast);
                    self.visit_type(&mut cast.ty);
                }
                self.ascend();
            }
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
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
        self.leave_expr(expr, is_operand);
    }

    /// The rewrites an expression takes once its operands have theirs.
    fn leave_expr(&mut self, expr: &mut Expression<'ast>, is_operand: bool) {
        let allocator = self.allocator;
        match expr {
            Expression::BinaryOp(_) => {
                if self.config.remove_dead_code && self.fold_redundant_ternary(expr) {
                    self.has_changed = true;
                    return self.leave_expr(expr, is_operand);
                }
                if self.config.fold_constants && self.fold(expr) {
                    self.has_changed = true;
                    return self.leave_expr(expr, is_operand);
                }
                if self.config.simplify_parens && simplify_parens::strip(allocator, expr) {
                    self.has_changed = true;
                }
            }
            Expression::UnaryOp(_) => {
                if self.config.fold_constants && self.fold(expr) {
                    self.has_changed = true;
                    return self.leave_expr(expr, is_operand);
                }
                if self.config.simplify_parens && simplify_parens::strip(allocator, expr) {
                    self.has_changed = true;
                }
            }
            Expression::Parenthesized(_) => {
                if self.config.simplify_parens
                    && if is_operand {
                        simplify_parens::strip(allocator, expr)
                    } else {
                        simplify_parens::strip_standalone(expr)
                    }
                {
                    self.has_changed = true;
                }
            }
            Expression::StringLiteral(literal) => {
                if self.config.shorten_strings
                    && self.respeller.shorten_string(allocator, self.version, literal)
                {
                    self.has_changed = true;
                }
            }
            Expression::Number(literal) => {
                if self.config.shorten_numbers
                    && self.respeller.shorten_number(allocator, self.subtypes, literal)
                {
                    self.has_changed = true;
                }
            }
            // Luau
            Expression::Integer(literal) => {
                if self.config.shorten_numbers && self.respeller.shorten_integer(allocator, literal)
                {
                    self.has_changed = true;
                }
            }
            // Luau
            Expression::TypeCast(_) => {
                if self.config.simplify_parens && simplify_parens::strip(allocator, expr) {
                    self.has_changed = true;
                }
            }
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_) => {}
        }
    }

    /// Folds `expr` and uncounts the operands the fold drops. A literal the
    /// fold leaves at either end of a run, or under a `-`, is one the walk
    /// never reaches, so it takes its rewrites here, and the caller's take
    /// a whole folded literal.
    fn fold(&mut self, expr: &mut Expression<'ast>) -> bool {
        let (allocator, version) = (self.allocator, self.version);
        let mut counter = self.counter();
        if !fold_constants::fold(allocator, version, expr, &mut |dropped| {
            counter.visit_expr(dropped);
        }) {
            return false;
        }
        match expr {
            Expression::BinaryOp(run) => {
                self.leave_literal(&mut run.first);
                self.leave_literal(&mut run.links.last_mut().operand);
            }
            Expression::UnaryOp(_) => self.leave_literal(expr),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => {}
        }
        true
    }

    /// The rewrites of a folded literal, a negative number's numeral
    /// included.
    fn leave_literal(&mut self, expr: &mut Expression<'ast>) {
        match expr {
            Expression::Number(_) | Expression::StringLiteral(_) => self.leave_expr(expr, true),
            Expression::UnaryOp(negation) if matches!(negation.operand, Expression::Number(_)) => {
                self.leave_expr(&mut negation.operand, true);
            }
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Integer(_)
            | Expression::UnaryOp(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::BinaryOp(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => {}
        }
    }
}

impl<'ast> VisitMut<'ast> for Compressor<'_, 'ast> {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        true
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_block(&mut self, block: &mut Block<'ast>) {
        self.descend();
        let outer_registers = self.live_registers;
        self.visit_stmts(&mut block.stmts);
        self.finish_block(block, outer_registers);
        self.ascend();
    }

    /// Every place the default walks reach an expression from is one where
    /// it stands alone. Operands and chain prefixes have walks of their own.
    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        self.visit_expr_in(expr, false);
    }

    /// The prefix's own parentheses aren't the walk's to strip, since a
    /// literal such as `("s"):rep(2)` needs them.
    fn visit_suffixed_expr(&mut self, suffixed_expr: &mut SuffixedExpression<'ast>) {
        if self.descend() {
            if let Expression::Parenthesized(parenthesized) = &mut suffixed_expr.prefix {
                if self.descend() {
                    self.visit_expr(&mut parenthesized.expr);
                }
                self.ascend();
            } else {
                self.visit_expr(&mut suffixed_expr.prefix);
                // Only an inlined value can leave a prefix that can't stand
                // bare, as `x["k"]` with `x = 37.5` becomes `(37.5)["k"]`.
                if !is_prefix_expr(&suffixed_expr.prefix) {
                    suffixed_expr.prefix =
                        parenthesize(self.allocator, suffixed_expr.prefix.take());
                }
            }
            for suffix in &mut *suffixed_expr.suffixes {
                match suffix {
                    Suffix::Field(_) => {}
                    Suffix::Index { index, .. } => {
                        self.visit_expr(index);
                        if self.config.simplify_indexes
                            && simplify_indexes::index_to_field(suffix, self.version)
                        {
                            self.has_changed = true;
                        }
                    }
                    Suffix::Call(args) => self.visit_function_args(args),
                    Suffix::MethodCall(call) => {
                        if let Some(type_args) = &mut call.type_args {
                            for ty in &mut type_args.args {
                                self.visit_type(ty);
                            }
                        }
                        self.visit_function_args(&mut call.args);
                    }
                    // Luau
                    Suffix::TypeInstantiation { type_args, .. } => {
                        for ty in &mut type_args.args {
                            self.visit_type(ty);
                        }
                    }
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
        self.walk_function_args(function_args);
        if self.config.simplify_parens && simplify_parens::strip_call_args(function_args) {
            self.has_changed = true;
        }
        if self.config.shorten_strings
            && let FunctionArgs::StringLiteral(literal) = function_args
            && self.respeller.shorten_string(self.allocator, self.version, literal)
        {
            self.has_changed = true;
        }
    }

    fn visit_table_constructor(&mut self, table_constructor: &mut TableConstructor<'ast>) {
        self.walk_table_constructor(table_constructor);
        if self.config.simplify_indexes
            && simplify_indexes::simplify_table(table_constructor, self.version)
        {
            self.has_changed = true;
        }
    }

    /// The signature belongs to the scope around the function, as the bind
    /// walk counts it.
    fn visit_function_body(&mut self, function_body: &mut FunctionBody<'ast>) {
        let outer_registers =
            mem::replace(&mut self.live_registers, parameter_locals(function_body));
        if self.descend() {
            if let Some(generics) = &mut function_body.generics {
                self.visit_generic_type_list(generics);
            }
            for param in &mut function_body.params {
                if let Some(ty) = &mut param.type_annotation {
                    self.visit_type(ty);
                }
            }
            if let Some(ty) =
                function_body.vararg.as_mut().and_then(|vararg| vararg.type_annotation.as_mut())
            {
                self.visit_type(ty);
            }
            if let Some(ty) = &mut function_body.return_type {
                self.visit_type(ty);
            }
            self.function_depth = self.function_depth.saturating_add(1);
            let outer_has_changed = mem::take(&mut self.has_changed);
            self.visit_block(&mut function_body.block);
            if self.config.remove_dead_code {
                self.has_changed |= remove_dead_code::drop_bare_return(&mut function_body.block);
            }
            if self.config.lift_locals {
                self.lift(function_body);
            }
            self.has_changed |= outer_has_changed;
            self.function_depth = self.function_depth.saturating_sub(1);
        }
        self.ascend();
        self.live_registers = outer_registers;
    }

    fn visit_type(&mut self, ty: &mut Type<'ast>) {
        let outer_floor = mem::replace(&mut self.type_floor, self.function_depth.saturating_add(1));
        self.walk_type(ty);
        self.type_floor = outer_floor;
    }
}
