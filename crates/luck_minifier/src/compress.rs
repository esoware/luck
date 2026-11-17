//! The compressor: one in-place walk that runs every rewrite the tree
//! allows as it leaves each node, repeated until a walk changes nothing.
//!
//! A walk folds constants, drops dead branches and dead locals, inlines
//! single-use locals, merges neighboring locals and assignments, turns
//! `if ... return` runs into `and`/`or` chains, shortens literals and
//! indexes, and strips parentheses. Each rewrite reports what it changed, so
//! a walk that reports nothing is the fixpoint. Rewrites build their nodes
//! in the shape the parser gives the text they print, which is what makes
//! the fixpoint's own output a fixpoint.

mod control;
mod fold;
mod indexes;
mod literals;
mod locals;
mod parens;

use core::mem;

use luck_allocator::{Allocator, List};
use luck_ast::{
    BinaryOp, Block, Expression, FunctionArgs, FunctionBody, MAX_NESTING_DEPTH, Statement, Suffix,
    SuffixedExpression, TableConstructor, VisitorMut, block_nesting_depth, is_prefix_expr,
};
use luck_token::{LuaVersion, NumberSubtypes};
use rustc_hash::{FxHashMap, FxHashSet};

use self::literals::Literals;
use self::locals::{OperandPosition, Value};
use crate::expr::{constant_truthiness, inner_operand_side, parenthesize};
use crate::limits::{GENERIC_FOR_STATE_LOCALS, NUMERIC_FOR_STATE_LOCALS, parameter_locals};
use crate::name_table::NameTable;
use crate::scan::{Scan, is_droppable_function};
use crate::transform_config::TransformConfig;

/// Measured corpora settle in two or three walks, so the cap only bounds a
/// rewrite that never stops changing the tree, which the fixpoint tests
/// catch.
const MAX_WALKS: usize = 16;

pub(crate) struct Compressor<'ast> {
    allocator: &'ast Allocator,
    version: LuaVersion,
    subtypes: NumberSubtypes,
    config: TransformConfig,
    has_changed: bool,
    /// Levels of nesting open where the walk is.
    depth: usize,
    /// Every name the passes have met, which renaming and lifting share.
    names: NameTable<'ast>,
    scan: Scan,
    /// Whether `scan` already holds the next walk's facts.
    is_scanned: bool,
    /// Each inlined local's value by its name's id, waiting for its one read.
    values: FxHashMap<usize, Value<'ast>>,
    /// Whether a value has landed bare as an operand since the run being
    /// walked began, so the run may hold one to splice.
    has_bare_operand: bool,
    /// The inlined locals taken in by the values landed since the
    /// initializer being walked began.
    landed_locals: usize,
    /// Each initializer's `landed_locals`, and each name's scan facts, for
    /// the `local`s being walked, innermost last.
    landed_counts: Vec<usize>,
    local_facts: Vec<(bool, Option<usize>)>,
    /// The ids of the declared locals and parameters in scope, upvalues
    /// included.
    locals: Vec<usize>,
    /// The ids of the locals the block being merged declares, in order, and
    /// the names of the group being merged.
    block_locals: Vec<usize>,
    group_names: Vec<&'ast str>,
    live_registers: usize,
    /// Lua 5.5: every name a `global` statement has declared so far, which
    /// stops counting as a local even past that statement's block.
    global_names: FxHashSet<&'ast str>,
    /// The calls an unused `local` leaves past the first, to follow it.
    extracted_calls: Vec<Statement<'ast>>,
    /// A block's statements while `do` blocks in it give up theirs.
    unwrapped_stmts: Vec<Statement<'ast>>,
    literals: Literals,
}

impl<'ast> Compressor<'ast> {
    pub(crate) fn new(
        allocator: &'ast Allocator,
        version: LuaVersion,
        config: TransformConfig,
    ) -> Self {
        Self {
            allocator,
            version,
            subtypes: NumberSubtypes::from(version),
            config,
            has_changed: false,
            depth: 0,
            names: NameTable::new(),
            scan: Scan::default(),
            is_scanned: false,
            values: FxHashMap::default(),
            has_bare_operand: false,
            landed_locals: 0,
            landed_counts: Vec::new(),
            local_facts: Vec::new(),
            locals: Vec::new(),
            block_locals: Vec::new(),
            group_names: Vec::new(),
            live_registers: 0,
            global_names: FxHashSet::default(),
            extracted_calls: Vec::new(),
            unwrapped_stmts: Vec::new(),
            literals: Literals::default(),
        }
    }

    pub(crate) const fn names(&mut self) -> &mut NameTable<'ast> {
        &mut self.names
    }

    /// Walks until a walk changes nothing, and says whether any did.
    pub(crate) fn compress(&mut self, block: &mut Block<'ast>) -> bool {
        let mut has_changed = false;
        for _ in 0..MAX_WALKS {
            if !self.walk(block) {
                break;
            }
            has_changed = true;
        }
        has_changed
    }

    /// Whether `block` nests within [`MAX_NESTING_DEPTH`], found by the first
    /// walk's scan where there is one.
    pub(crate) fn fits_nesting_cap(&mut self, block: &mut Block<'ast>) -> bool {
        let should_find_dead = self.config.remove_dead_code;
        let should_find_inlinable = self.config.inline_locals;
        if !should_find_dead && !should_find_inlinable {
            return block_nesting_depth(block) <= MAX_NESTING_DEPTH;
        }
        self.is_scanned = true;
        self.scan.run(block, &mut self.names, self.version, should_find_dead, should_find_inlinable)
            <= MAX_NESTING_DEPTH
    }

    fn walk(&mut self, block: &mut Block<'ast>) -> bool {
        let should_find_dead = self.config.remove_dead_code;
        let should_find_inlinable = self.config.inline_locals;
        if (should_find_dead || should_find_inlinable) && !mem::take(&mut self.is_scanned) {
            self.scan.run(
                block,
                &mut self.names,
                self.version,
                should_find_dead,
                should_find_inlinable,
            );
        }
        self.has_changed = false;
        self.depth = 0;
        self.values.clear();
        self.locals.clear();
        self.live_registers = 0;
        self.global_names.clear();
        self.visit_block(block);
        // The chunk is a function too, and returns nothing either way.
        if self.config.remove_dead_code && control::ends_in_bare_return(block) {
            block.last_stmt = None;
            self.has_changed = true;
        }
        self.has_changed
    }

    /// Walks each statement, drops the ones the walk leaves nothing of, and
    /// puts the calls an unused `local` leaves in its place.
    fn visit_stmts(&mut self, stmts: &mut List<'ast, Statement<'ast>>) {
        let mut kept = 0usize;
        let mut index = 0usize;
        while let Some(stmt) = stmts.get_mut(index) {
            if self.visit_kept_stmt(stmt) {
                stmts.swap(kept, index);
                if let Some(kept_stmt) = stmts.get(kept) {
                    self.declare_locals(kept_stmt);
                }
                kept = kept.saturating_add(1);
            } else {
                self.has_changed = true;
            }
            index = index.saturating_add(1);
            let allocator = self.allocator;
            for call in self.extracted_calls.drain(..) {
                stmts.insert(kept, call, allocator);
                kept = kept.saturating_add(1);
                index = index.saturating_add(1);
            }
        }
        stmts.truncate(kept);
    }

    /// Walks `stmt` and rewrites it, and says whether anything of it stays.
    fn visit_kept_stmt(&mut self, stmt: &mut Statement<'ast>) -> bool {
        let should_remove_dead_code = self.config.remove_dead_code;
        match stmt {
            Statement::LocalAssignment(_) => return self.visit_local(stmt),
            Statement::LocalFunction(local_function) => {
                if should_remove_dead_code
                    && is_droppable_function(local_function, self.version)
                    && !self.scan.is_live(self.names.find(local_function.name.text))
                {
                    return false;
                }
            }
            Statement::IfStatement(_) => return self.visit_if(stmt),
            Statement::WhileLoop(while_loop) => {
                self.visit_expr(&mut while_loop.condition);
                if should_remove_dead_code
                    && constant_truthiness(&while_loop.condition) == Some(false)
                {
                    return false;
                }
                self.visit_block(&mut while_loop.block);
                return true;
            }
            Statement::RepeatLoop(_) => return self.visit_repeat(stmt),
            Statement::DoBlock(do_block) => {
                self.visit_block(&mut do_block.block);
                return !(should_remove_dead_code && control::is_empty_block(&do_block.block));
            }
            Statement::NumericFor(numeric_for) => {
                if should_remove_dead_code && control::has_default_step(numeric_for) {
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
            // A Lua 5.2+ `;` never decides whether a label ends its block,
            // since every Lua skips past it to see, and the printer puts one
            // back before a statement that starts with `(`.
            Statement::EmptyStatement(_) => return !should_remove_dead_code,
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                self.global_names
                    .extend(global.names.iter().map(|attributed| attributed.name.text));
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.global_names.insert(global_function.name.text);
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
        true
    }

    /// Records the locals a kept statement declares, for the assignments
    /// after it that merging has to prove assign locals.
    fn declare_locals(&mut self, stmt: &Statement<'ast>) {
        if let Statement::LocalAssignment(local) = stmt {
            for attributed in &local.names {
                let id = self.names.intern_text(attributed.name.text);
                self.locals.push(id);
            }
        }
        if let Statement::LocalFunction(local_function) = stmt {
            let id = self.names.intern_text(local_function.name.text);
            self.locals.push(id);
        }
        self.live_registers = self.live_registers.saturating_add(declared_count(stmt));
    }

    /// [`Self::declare_locals`] again for the block being merged, from the
    /// ids its walk found, the next of which is at `next_local`.
    fn redeclare_locals(&mut self, stmt: &Statement<'ast>, next_local: &mut usize) {
        let count = declared_count(stmt);
        let end = next_local.saturating_add(count);
        self.locals.extend_from_slice(self.block_locals.get(*next_local..end).unwrap_or_default());
        *next_local = end;
        self.live_registers = self.live_registers.saturating_add(count);
    }

    /// Walks a run's operands, landing an inlined value bare wherever it
    /// keeps its grouping, and splices a run that lands bare at the run's
    /// own level.
    fn walk_run(&mut self, run: &mut BinaryOp<'ast>) {
        if self.descend() {
            if !self.values.is_empty() {
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
                if !self.values.is_empty()
                    && let Some(value) = self.take_value(name.text)
                {
                    *expr = value;
                }
                return;
            }
            Expression::BinaryOp(run) => self.walk_run(run),
            Expression::UnaryOp(unary_op) => {
                if self.descend() {
                    self.visit_operand(&mut unary_op.operand, OperandPosition::Unary);
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
                if self.config.remove_dead_code && control::fold_redundant_ternary(allocator, expr)
                {
                    self.has_changed = true;
                    return self.leave_expr(expr, is_operand);
                }
                if self.config.fold_constants && fold::fold(allocator, self.version, expr) {
                    self.has_changed = true;
                    return self.leave_expr(expr, is_operand);
                }
                if self.config.simplify_parens && parens::strip(allocator, expr) {
                    self.has_changed = true;
                }
            }
            Expression::UnaryOp(_) => {
                if self.config.fold_constants && fold::fold(allocator, self.version, expr) {
                    self.has_changed = true;
                    return self.leave_expr(expr, is_operand);
                }
                if self.config.simplify_parens && parens::strip(allocator, expr) {
                    self.has_changed = true;
                }
            }
            Expression::Parenthesized(_) => {
                if self.config.simplify_parens
                    && if is_operand {
                        parens::strip(allocator, expr)
                    } else {
                        parens::strip_standalone(expr)
                    }
                {
                    self.has_changed = true;
                }
            }
            Expression::StringLiteral(literal) => {
                if self.config.shorten_strings
                    && self.literals.shorten_string(allocator, self.version, literal)
                {
                    self.has_changed = true;
                }
            }
            Expression::Number(literal) => {
                if self.config.shorten_numbers
                    && self.literals.shorten_number(allocator, self.subtypes, literal)
                {
                    self.has_changed = true;
                }
            }
            // Luau
            Expression::Integer(literal) => {
                if self.config.shorten_numbers && self.literals.shorten_integer(allocator, literal)
                {
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
            | Expression::TypeCast(_)
            | Expression::Error(_) => {}
        }
    }
}

/// How many locals `stmt` declares.
fn declared_count(stmt: &Statement<'_>) -> usize {
    match stmt {
        Statement::LocalAssignment(local) => local.names.len(),
        Statement::LocalFunction(_) => 1,
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
        | Statement::Error(_) => 0,
    }
}

impl<'ast> VisitorMut<'ast> for Compressor<'ast> {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        true
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_block(&mut self, block: &mut Block<'ast>) {
        self.descend();
        let scope_start = self.locals.len();
        let outer_registers = self.live_registers;
        self.visit_stmts(&mut block.stmts);
        if let Some(last) = &mut block.last_stmt {
            self.visit_last_stmt(last);
        }
        if self.config.simplify_statements {
            self.unwrap_do_blocks(block);
        }
        if self.config.merge_locals {
            // Merging proves each assignment's target local as of its own
            // statement, so the block's locals are counted again in order.
            self.block_locals.clear();
            self.block_locals.extend_from_slice(self.locals.get(scope_start..).unwrap_or_default());
            self.locals.truncate(scope_start);
            self.live_registers = outer_registers;
            self.merge_stmts(&mut block.stmts);
        }
        if self.config.simplify_statements {
            self.convert_if_return_chain(block);
        }
        self.locals.truncate(scope_start);
        self.live_registers = outer_registers;
        self.ascend();
    }

    /// Every place the default walks reach an expression from is one where
    /// it stands alone. Operands and chain prefixes have walks of their own.
    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        self.visit_expr_in(expr, false);
    }

    /// The prefix's own parentheses aren't the walk's to strip, since a
    /// literal such as `("s"):rep(2)` needs them.
    fn visit_suffixed(&mut self, chain: &mut SuffixedExpression<'ast>) {
        if self.descend() {
            if let Expression::Parenthesized(parenthesized) = &mut chain.prefix {
                if self.descend() {
                    self.visit_expr(&mut parenthesized.expr);
                }
                self.ascend();
            } else {
                self.visit_expr(&mut chain.prefix);
                // Only an inlined value can leave a prefix that can't stand
                // bare, as `x["k"]` with `x = 37.5` becomes `(37.5)["k"]`.
                if !is_prefix_expr(&chain.prefix) {
                    chain.prefix = parenthesize(self.allocator, chain.prefix.take());
                }
            }
            for suffix in &mut *chain.suffixes {
                match suffix {
                    Suffix::Field(_) => {}
                    Suffix::Index { index, .. } => {
                        self.visit_expr(index);
                        if self.config.simplify_indexes
                            && indexes::index_to_field(suffix, self.version)
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
            if self.config.simplify_parens && parens::strip_prefix(self.allocator, chain) {
                self.has_changed = true;
            }
        }
        self.ascend();
    }

    fn visit_function_args(&mut self, args: &mut FunctionArgs<'ast>) {
        self.walk_function_args(args);
        if self.config.simplify_parens && parens::strip_call_args(args) {
            self.has_changed = true;
        }
        if self.config.shorten_strings
            && let FunctionArgs::StringLiteral(literal) = args
            && self.literals.shorten_string(self.allocator, self.version, literal)
        {
            self.has_changed = true;
        }
    }

    fn visit_table_constructor(&mut self, table: &mut TableConstructor<'ast>) {
        self.walk_table_constructor(table);
        if self.config.simplify_indexes && indexes::simplify_table(table, self.version) {
            self.has_changed = true;
        }
    }

    fn visit_function_body(&mut self, body: &mut FunctionBody<'ast>) {
        let outer_registers = mem::replace(&mut self.live_registers, parameter_locals(body));
        let outer_local_count = self.locals.len();
        for param in &body.params {
            let id = self.names.intern_text(param.name.text);
            self.locals.push(id);
        }
        self.walk_function_body(body);
        self.locals.truncate(outer_local_count);
        self.live_registers = outer_registers;
        if self.config.remove_dead_code && control::ends_in_bare_return(&body.block) {
            body.block.last_stmt = None;
            self.has_changed = true;
        }
    }
}
