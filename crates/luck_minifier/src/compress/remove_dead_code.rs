//! Dropping code that never runs or does nothing.
//!
//! That's constant branches and the `if`s they leave, loops that run once or
//! never, a default `for` step, a bare closing `return`, `cond and X or X`,
//! and `local`s nothing kept uses.
//!
//! A local is referenced only in its block, so the walk drops a `local`
//! nothing references as it reaches it, and those its rewrites leave
//! unreferenced as it leaves the block, last first, since each one's
//! initializer can hold the last reference to one before it.

use std::mem;

use luck_allocator::{Allocator, List};
use luck_ast::{
    BinaryOp, Block, ContentEq as _, ElseClause, ElseIfClause, Expression, GetSpan as _,
    IfStatement, LocalAssignment, NumericFor, OneOrMore, Statement, VisitMut as _,
};
use luck_token::{BinOp, LuaNumber, UnOp};
use luck_value::{ConstantEvaluation as _, ConstantValue, MayHaveSideEffects as _};

use super::simplify_parens::Place;
use super::simplify_statements::{has_no_else, holds_nested_if};
use super::{Compressor, remove_stmts};
use crate::binding::{
    ResolvedNames, has_attributed_or_env_name, has_fixed_binding, is_droppable_function,
};
use crate::expr::{fits_at_depth, truncate_multi_value};

pub(super) fn is_empty_block(block: &Block<'_>) -> bool {
    block.stmts.is_empty()
}

/// A step of integer 1 is the default. On Lua 5.3+ a float step makes the
/// loop a float one, so `1.0` isn't, but where every number is a float, it is.
pub(super) fn has_default_step(numeric_for: &NumericFor<'_>, names: ResolvedNames<'_>) -> bool {
    let default = if names.0.version().has_integer_subtype() {
        LuaNumber::Int(1)
    } else {
        LuaNumber::Float(1.0)
    };
    numeric_for.step.as_ref().and_then(|step| step.evaluate_value(&names))
        == Some(ConstantValue::Number(default))
}

/// Drops the `return` that ends `block`, a function's or the chunk's, when
/// it's bare, and says whether it did. `return nil` returns one value, and
/// `select('#', f())` observes the difference.
pub(super) fn drop_bare_return(block: &mut Block<'_>) -> bool {
    let is_bare = matches!(
        block.stmts.last(),
        Some(Statement::Return(return_stmt)) if return_stmt.exprs.is_empty()
    );
    if is_bare {
        block.stmts.truncate(block.stmts.len() - 1);
    }
    is_bare
}

/// Nested loops and function bodies aren't searched, since their exits can't
/// reach this loop.
fn has_loop_exit(block: &Block<'_>) -> bool {
    block.stmts.iter().any(|stmt| match stmt {
        Statement::Break(_) | Statement::Continue(_) => true,
        Statement::DoBlock(do_block) => has_loop_exit(&do_block.block),
        Statement::IfStatement(if_stmt) => {
            has_loop_exit(&if_stmt.block)
                || if_stmt.elseif_clauses.iter().any(|clause| has_loop_exit(&clause.block))
                || if_stmt.else_clause.as_ref().is_some_and(|clause| has_loop_exit(&clause.block))
        }
        Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::GlobalFunction(_)
        | Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::LocalAssignment(_)
        | Statement::Return(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalStar(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    })
}

fn do_block<'ast>(allocator: &'ast Allocator, block: Block<'ast>) -> Statement<'ast> {
    let span = block.span;
    Statement::new_do_block(span, block, allocator)
}

/// Only wraps in `not`. Inverting a comparison operator would change which
/// metamethod is called, `__lt` instead of `__le` and so on.
fn negate<'ast>(allocator: &'ast Allocator, expr: Expression<'ast>) -> Expression<'ast> {
    let span = expr.span();
    Expression::new_unary_op(span, UnOp::Not, expr, allocator)
}

/// `cond and X or X`, which a run of `or`s starts with when its first two
/// operands are that.
fn is_redundant_ternary(outer: &BinaryOp<'_>, names: ResolvedNames<'_>) -> bool {
    let Expression::BinaryOp(inner) = &outer.first else {
        return false;
    };
    let alternative = &outer.links.first().operand;
    let conditions = &inner.links[..inner.links.len() - 1];
    outer.links.first().op == BinOp::Or
        && inner.links.first().op == BinOp::And
        && inner.links.last().operand.content_eq(alternative)
        && !inner.first.may_have_side_effects(&names)
        && conditions.iter().all(|link| !link.operand.may_have_side_effects(&names))
        // X must be pure too: a call that returns a falsy value runs twice in
        // the original but once here.
        && !alternative.may_have_side_effects(&names)
}

/// What becomes of a `local` when none of its names is used.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UnusedLocalAction {
    Keep,
    Remove,
    /// Every initializer is pure or a call, so only the calls stay.
    ExtractCalls,
}

/// The rewrites can't make a value less pure, so a `local` this finds
/// droppable before a walk is droppable after it.
pub(super) fn unused_local_action(
    local: &LocalAssignment<'_>,
    names: ResolvedNames<'_>,
) -> UnusedLocalAction {
    // Exports, attributed names, and a 5.2+ `_ENV` are never dead.
    if local.kind.is_exported() || has_attributed_or_env_name(local, names.0.version()) {
        return UnusedLocalAction::Keep;
    }
    let is_pure = |expr: &Expression<'_>| !expr.may_have_side_effects(&names);
    if local.exprs.iter().all(is_pure) {
        UnusedLocalAction::Remove
    } else if local.exprs.iter().all(|expr| is_pure(expr) || is_call(expr)) {
        UnusedLocalAction::ExtractCalls
    } else {
        UnusedLocalAction::Keep
    }
}

fn is_call(expr: &Expression<'_>) -> bool {
    matches!(expr, Expression::Suffixed(chain) if chain.is_call())
}

/// What of a statement goes.
enum Dropped {
    Nothing,
    Part,
    Whole,
}

/// `local x = nil` can become `local x`, since nil is the default. Const
/// declarations keep their mandatory initializer.
pub(super) fn is_nil_initialized(local: &LocalAssignment<'_>) -> bool {
    !local.kind.is_const()
        && local.names.len() == 1
        && matches!(&*local.exprs, [Expression::Nil(_)])
}

impl<'ast> Compressor<'_, 'ast> {
    /// Drops what of `stmts`, a block the walk is leaving, nothing
    /// references.
    pub(super) fn drop_unused(&mut self, stmts: &mut List<'ast, Statement<'ast>>) {
        let mut has_dropped = false;
        // The statements that go whole, last first.
        let mut removed = mem::take(&mut self.removed_stmts);
        removed.clear();
        for (index, stmt) in stmts.iter_mut().enumerate().rev() {
            match self.drop_unused_from(stmt, index) {
                Dropped::Nothing => {}
                Dropped::Part => has_dropped = true,
                Dropped::Whole => {
                    removed.push(index);
                    has_dropped = true;
                }
            }
        }
        if has_dropped {
            self.has_changed = true;
            removed.reverse();
            if self.late_calls.is_empty() {
                remove_stmts(stmts, &removed);
            } else {
                self.insert_late_calls(stmts, &removed);
            }
        }
        self.removed_stmts = removed;
    }

    /// Drops the statements at `removed`, ascending, and puts the calls an
    /// unused `local` leaves after the one that stands in its place.
    fn insert_late_calls(&mut self, stmts: &mut List<'ast, Statement<'ast>>, removed: &[usize]) {
        let mut calls = self.late_calls.drain(..).rev().peekable();
        let mut to_drop = removed.iter().peekable();
        let mut kept = Vec::with_capacity(stmts.len() + calls.len());
        for (index, stmt) in mem::take(stmts).into_iter().enumerate() {
            if to_drop.next_if_eq(&&index).is_none() {
                kept.push(stmt);
            }
            while let Some((_, call)) = calls.next_if(|&(call_index, _)| call_index == index) {
                kept.push(call);
            }
        }
        *stmts = List::from_iter_in(kept, self.allocator);
    }

    /// Drops what of the statement at `index` nothing references.
    fn drop_unused_from(&mut self, stmt: &mut Statement<'ast>, index: usize) -> Dropped {
        match stmt {
            Statement::LocalFunction(local_function) => {
                if is_droppable_function(local_function, self.version)
                    && !self.is_used(&local_function.name)
                {
                    self.drop_stmt(stmt);
                    return Dropped::Whole;
                }
                Dropped::Nothing
            }
            Statement::LocalAssignment(local) => {
                if local.names.iter().all(|attributed| !self.is_used(&attributed.name)) {
                    match unused_local_action(local, self.names()) {
                        UnusedLocalAction::Keep => {}
                        UnusedLocalAction::Remove => {
                            self.drop_stmt(stmt);
                            return Dropped::Whole;
                        }
                        UnusedLocalAction::ExtractCalls => {
                            self.extract_calls(stmt, index);
                            return Dropped::Part;
                        }
                    }
                }
                // Dropping a name and its value keeps every other name's
                // value only when each name has its own.
                if has_fixed_binding(local, self.version) || local.names.len() != local.exprs.len()
                {
                    return Dropped::Nothing;
                }
                let mut kept = 0;
                for i in 0..local.names.len() {
                    let (attributed, value) = (&local.names[i], &local.exprs[i]);
                    if !self.is_used(&attributed.name)
                        && !value.may_have_side_effects(&self.names())
                    {
                        self.drop_annotation(attributed);
                        self.drop_expr(value);
                    } else {
                        local.names.swap(kept, i);
                        local.exprs.swap(kept, i);
                        kept += 1;
                    }
                }
                let has_dropped = kept < local.names.len();
                local.names.truncate(kept);
                local.exprs.truncate(kept);
                match (kept, has_dropped) {
                    (0, _) => Dropped::Whole,
                    (_, true) => Dropped::Part,
                    (_, false) => Dropped::Nothing,
                }
            }
            Statement::Assignment(_)
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
            | Statement::Error(_) => Dropped::Nothing,
        }
    }

    /// Puts the calls of an unused `local`, the statement at `index`, whose
    /// other initializers are pure in its place.
    fn extract_calls(&mut self, stmt: &mut Statement<'ast>, index: usize) {
        let Statement::LocalAssignment(local) = stmt else {
            return;
        };
        for attributed in &local.names {
            self.drop_annotation(attributed);
        }
        let mut first_call = None;
        let calls_start = self.late_calls.len();
        for expr in &mut local.exprs {
            if !is_call(expr) {
                self.drop_expr(expr);
                continue;
            }
            if let Expression::Suffixed(chain) = expr.take() {
                let call = Statement::FunctionCall(chain);
                if first_call.is_none() {
                    first_call = Some(call);
                } else {
                    self.late_calls.push((index, call));
                }
            }
        }
        // The calls go in last first, as the block's statements do.
        self.late_calls[calls_start..].reverse();
        if let Some(call) = first_call {
            *stmt = call;
        }
    }

    /// `cond and X or X` folds to `X` when both branches are written alike
    /// and the condition is pure. Says whether it folded.
    pub(super) fn fold_redundant_ternary(&mut self, expr: &mut Expression<'ast>) -> bool {
        let Expression::BinaryOp(outer) = expr else {
            return false;
        };
        if !is_redundant_ternary(outer, self.names()) {
            return false;
        }
        self.drop_expr(&outer.first);
        let allocator = self.allocator;
        let kept = outer.links[0].operand.take();
        match OneOrMore::from_list(outer.links.split_off_rest()) {
            Some(rest) => {
                outer.first = kept;
                outer.links = rest;
            }
            None => *expr = truncate_multi_value(allocator, kept),
        }
        true
    }

    /// Walks the condition of an `if`'s arm, and says whether it's truthy
    /// when that's known and it can go, which it then does, dropped: the arm
    /// runs whenever the `if` reaches it, or never.
    fn visit_arm_condition(&mut self, condition: &mut Expression<'ast>) -> Option<bool> {
        self.visit_expr(condition);
        let truthiness = condition.get_side_free_boolean_value(&self.names());
        if truthiness.is_some() {
            self.drop_expr(condition);
        }
        truthiness
    }

    /// Drops the arms of an `if` the walk leaves unwalked.
    fn drop_arms(
        &mut self,
        clauses: &[ElseIfClause<'ast>],
        else_clause: Option<&ElseClause<'ast>>,
    ) {
        for clause in clauses {
            self.drop_expr(&clause.condition);
            self.drop_block(&clause.block);
        }
        if let Some(clause) = else_clause {
            self.drop_block(&clause.block);
        }
    }

    /// Walks an `if`, keeping only the arms that can run and walking only
    /// those, then trades branches, merges a nested `if`, or drops what's
    /// left. Says whether anything of it stays.
    ///
    /// An arm under a falsy condition that can go never runs. The first under
    /// a truthy one runs whenever it's reached, so it becomes the `else` and
    /// every arm after it goes.
    pub(super) fn visit_if(&mut self, stmt: &mut Statement<'ast>) -> bool {
        let Statement::IfStatement(if_stmt) = stmt else {
            return true;
        };
        let is_in_run = mem::take(&mut self.is_in_if_run);
        let holds_run = holds_nested_if(if_stmt);
        if !self.config.remove_dead_code {
            self.walk_if(if_stmt, holds_run);
            if !is_in_run {
                self.merge_nested_ifs(if_stmt, self.depth);
            }
            return true;
        }
        let IfStatement { condition, block, elseif_clauses, else_clause, .. } = &mut **if_stmt;
        let mut has_head = match self.visit_arm_condition(condition) {
            Some(true) => {
                self.visit_run_block(block, holds_run);
                // The `if`s under it left their merge to this one, which goes.
                if holds_run && let [Statement::IfStatement(inner)] = &mut *block.stmts {
                    self.merge_nested_ifs(inner, self.depth + 1);
                }
                self.drop_arms(elseif_clauses, else_clause.as_deref());
                *stmt = do_block(self.allocator, block.take());
                self.has_changed = true;
                return !matches!(stmt, Statement::DoBlock(do_block) if is_empty_block(&do_block.block));
            }
            Some(false) => {
                self.drop_block(block);
                false
            }
            None => {
                self.visit_run_block(block, holds_run);
                true
            }
        };
        let mut has_dropped_arm = !has_head;
        let mut taken_block = None;
        let mut kept = 0;
        let mut taken_index = None;
        for index in 0..elseif_clauses.len() {
            let clause = &mut elseif_clauses[index];
            let truthiness = self.visit_arm_condition(&mut clause.condition);
            has_dropped_arm |= truthiness.is_some();
            match truthiness {
                Some(false) => self.drop_block(&clause.block),
                Some(true) => {
                    taken_block = Some(clause.block.take());
                    taken_index = Some(index);
                    break;
                }
                None if has_head => {
                    self.visit_block(&mut clause.block);
                    elseif_clauses.swap(kept, index);
                    kept += 1;
                }
                // The first arm left takes the dropped head's place.
                None => {
                    has_head = true;
                    *condition = clause.condition.take();
                    self.visit_block(&mut clause.block);
                    *block = clause.block.take();
                }
            }
        }
        if let Some(index) = taken_index {
            self.drop_arms(&elseif_clauses[index + 1..], else_clause.as_deref());
        }
        elseif_clauses.truncate(kept);
        if let Some(taken) = taken_block {
            *else_clause = Some(ElseClause::boxed(taken.span, taken, self.allocator));
        }
        if let Some(clause) = else_clause {
            self.visit_block(&mut clause.block);
        }
        if has_dropped_arm {
            self.has_changed = true;
        }
        if !has_head {
            return match else_clause.take() {
                Some(clause) => {
                    *stmt = do_block(self.allocator, clause.unbox().block);
                    !matches!(stmt, Statement::DoBlock(do_block) if is_empty_block(&do_block.block))
                }
                // `if false then ... end`.
                None => false,
            };
        }
        self.trade_branches(if_stmt);
        if !is_in_run {
            self.merge_nested_ifs(if_stmt, self.depth);
        }
        // An empty-body `if` can only go when evaluating the condition is
        // side-effect free: `if f() then end` calls f.
        let is_dead = has_no_else(if_stmt)
            && is_empty_block(&if_stmt.block)
            && !if_stmt.condition.may_have_side_effects(&self.names());
        if is_dead {
            self.drop_stmt(stmt);
        }
        !is_dead
    }

    /// An `if` whose one statement is an `if` merges the run that starts
    /// there, so the one inside leaves it.
    fn visit_run_block(&mut self, block: &mut Block<'ast>, holds_run: bool) {
        self.is_in_if_run = holds_run && self.config.simplify_statements;
        self.visit_block(block);
        self.is_in_if_run = false;
    }

    fn walk_if(&mut self, if_stmt: &mut IfStatement<'ast>, holds_run: bool) {
        self.visit_expr(&mut if_stmt.condition);
        self.visit_run_block(&mut if_stmt.block, holds_run);
        for clause in &mut if_stmt.elseif_clauses {
            self.visit_expr(&mut clause.condition);
            self.visit_block(&mut clause.block);
        }
        if let Some(else_clause) = &mut if_stmt.else_clause {
            self.visit_block(&mut else_clause.block);
        }
    }

    /// An `if` whose one `else` is empty loses it, or trades places with an
    /// empty `then` under the negated condition, or with the `then` of a
    /// negated condition.
    fn trade_branches(&mut self, if_stmt: &mut IfStatement<'ast>) {
        if !if_stmt.elseif_clauses.is_empty() {
            return;
        }
        let Some(else_clause) = &mut if_stmt.else_clause else {
            return;
        };
        if is_empty_block(&else_clause.block) {
            if_stmt.else_clause = None;
            self.has_changed = true;
            return;
        }
        // The `not` nests the condition a level deeper.
        if is_empty_block(&if_stmt.block) && fits_at_depth(self.depth, 1, &if_stmt.condition) {
            if_stmt.condition = negate(self.allocator, if_stmt.condition.take());
            if_stmt.block = else_clause.block.take();
            if_stmt.else_clause = None;
            self.leave_expr(&mut if_stmt.condition, Place::OneValue);
            self.has_changed = true;
            return;
        }
        // `if not c then A else B end` becomes `if c then B else A end`.
        while let Expression::UnaryOp(negation) = &mut if_stmt.condition
            && negation.op == UnOp::Not
        {
            if_stmt.condition = negation.operand.take();
            mem::swap(&mut if_stmt.block, &mut else_clause.block);
            self.has_changed = true;
        }
    }

    /// `repeat ... until true` runs once, so it becomes a `do` block unless
    /// its body has its own `break` or `continue`, which a `do` block would
    /// rebind to an outer loop (or make invalid). Says whether anything of it
    /// stays.
    ///
    /// The condition sees the body's locals, so the body's walk finishes
    /// after it.
    pub(super) fn visit_repeat(&mut self, stmt: &mut Statement<'ast>) -> bool {
        let Statement::RepeatLoop(repeat_loop) = stmt else {
            return true;
        };
        self.descend();
        let (outer_registers, held_start) = (self.live_registers, self.held.len());
        self.visit_stmts(&mut repeat_loop.block.stmts);
        self.ascend();
        self.visit_expr(&mut repeat_loop.condition);
        self.descend();
        self.finish_block(&mut repeat_loop.block, outer_registers, held_start);
        self.ascend();
        if !self.config.remove_dead_code
            || repeat_loop.condition.get_side_free_boolean_value(&self.names()) != Some(true)
            || has_loop_exit(&repeat_loop.block)
        {
            return true;
        }
        self.drop_expr(&repeat_loop.condition);
        let block = repeat_loop.block.take();
        self.has_changed = true;
        let is_empty = is_empty_block(&block);
        *stmt = do_block(self.allocator, block);
        !is_empty
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, assert_outputs, minify_with, walk_once};
    use crate::transform_config::TransformConfig;

    const DEAD_CODE: TransformConfig = TransformConfig { remove_dead_code: true, ..NO_PASSES };

    fn remove_dead(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &DEAD_CODE)
    }

    #[test]
    fn makes_each_rewrite_when_it_is_the_only_one() {
        for (source, expected) in [
            ("local function f() return end return f", "local function f()end return f"),
            ("for i = 1, 2, 1 do f(i) end", "for i=1,2 do f(i)end"),
            ("local x = nil return x", "local x return x"),
            ("repeat f() until true", "do f()end"),
            ("if not a then f() else g() end", "if a then g()else f()end"),
            ("if a then else f() end", "if not a then f()end"),
            ("if a then f() elseif 1 then g() end", "if a then f()else g()end"),
            ("if {} then end", ""),
            ("while nil do f() end", ""),
            ("do end", ""),
            ("local x = f()", "f()"),
            ("f(); g()", "f()g()"),
            ("return 1 < 2 and \"x\" or \"x\"", "return\"x\""),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_the_semicolon_a_leading_paren_needs() {
        assert_eq!(remove_dead("f(); (g or h)()"), "f();(g or h)()");
    }

    /// Every Lua skips a `;` to see whether a label ends its block, which
    /// lets a `goto` jump past a `local` to it.
    #[test]
    fn removes_a_semicolon_after_a_label_that_ends_its_block() {
        assert_eq!(
            remove_dead("do goto done local x = f() print(x) ::done:: ; end"),
            "do goto done local x=f()print(x)::done::end"
        );
    }

    #[test]
    fn removes_dead_branches() {
        for (source, expected) in [
            ("if false then print(1) end", ""),
            ("if true then print(1) end", "do print(1)end"),
            ("while false do print(1) end", ""),
            (
                "if false then print(1) elseif x then print(2) else print(3) end",
                "if x then print(2)else print(3)end",
            ),
            (
                "if false then print(1) elseif false then print(2) else print(3) end",
                "do print(3)end",
            ),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn drops_falsy_arms_anywhere_in_the_chain() {
        for (source, expected) in [
            (
                "if a then f(1) elseif false then f(2) elseif b then f(3) elseif nil then f(4) else f(5) end",
                "if a then f(1)elseif b then f(3)else f(5)end",
            ),
            ("if false then f(1) elseif nil then f(2) elseif a then f(3) end", "if a then f(3)end"),
            ("if false then f(1) elseif nil then f(2) else f(3) end", "do f(3)end"),
            ("if false then f(1) elseif nil then f(2) end", ""),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn turns_the_first_truthy_arm_into_the_else() {
        for (source, expected) in [
            (
                "if a then f(1) elseif 1 then f(2) elseif b then f(3) else f(4) end",
                "if a then f(1)else f(2)end",
            ),
            (
                "if false then f(1) elseif a then f(2) elseif 'x' then f(3) elseif true then f(4) end",
                "if a then f(2)else f(3)end",
            ),
            ("if false then f(1) elseif true then f(2) elseif a then f(3) end", "do f(2)end"),
            ("if true then f(1) elseif a then f(2) else f(3) end", "do f(1)end"),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_every_arm_whose_condition_is_not_a_literal() {
        let source = "if a then f(1) elseif b() then f(2) elseif c.d then f(3) end";
        assert_eq!(remove_dead(source), "if a then f(1)elseif b()then f(2)elseif c.d then f(3)end");
    }

    #[test]
    fn collapses_a_long_run_of_falsy_arms() {
        let source = format!(
            "if false then f(){} elseif a then g() end",
            " elseif false then f()".repeat(5_000)
        );
        assert_eq!(remove_dead(&source), "if a then g()end");
    }

    #[test]
    fn keeps_global_self_assignment() {
        // `x = x` on a global fires __index and __newindex under a metatabled
        // environment, and this pass has no binding info.
        assert_eq!(remove_dead("x = x return 1"), "x=x return 1");
    }

    #[test]
    fn unrolls_repeat_until_true_only_without_its_own_exit() {
        // `break` binds to the repeat; a do-block would rebind it to an
        // outer loop (or make it invalid).
        assert_eq!(remove_dead("repeat print(1) until true"), "do print(1)end");
        assert_eq!(
            remove_dead("while f() do repeat if g() then break end until true end"),
            "while f()do repeat if g()then break end until true end"
        );
        assert_eq!(remove_dead("repeat break until true"), "repeat break until true");
    }

    #[test]
    fn drops_a_bare_return_that_ends_the_chunk() {
        assert_eq!(remove_dead("print(1) return"), "print(1)");
        assert_eq!(remove_dead("return nil"), "return nil");
    }

    #[test]
    fn negating_a_condition_drops_the_truncation_it_no_longer_needs() {
        let config = TransformConfig { fold_constants: true, simplify_parens: true, ..DEAD_CODE };
        for source in ["if (f()) then else g() end", "if {} and (f()) then else g() end"] {
            assert_eq!(
                minify_with(source, LuaVersion::Lua54, &config),
                "if not f()then g()end",
                "source: {source}"
            );
        }
    }

    #[test]
    fn negated_empty_then_condition_keeps_its_grouping() {
        for (condition, expected) in [
            ("a == b", "if not(a==b)then print(1)end"),
            ("a and b", "if not(a and b)then print(1)end"),
            ("a or b", "if not(a or b)then print(1)end"),
        ] {
            assert_eq!(remove_dead(&format!("if {condition} then else print(1) end")), expected);
        }
    }

    #[test]
    fn folds_a_ternary_whose_branches_are_written_alike() {
        for (source, expected) in [
            ("return ... and 1 or 1\n", "return 1"),
            ("return ... and -2 ^ 3 or -2 ^ 3\n", "return-2^3"),
            ("return ... and {1, x = \"a\"} or {1, x = \"a\"}\n", "return{1,x=\"a\"}"),
            // Lua leaves unspecified whether two closures created apart are
            // the same value, so a program can't tell these apart.
            ("return ... and function() end or function() end\n", "return function()end"),
            ("return a and ... or ...\n", "return a and...or..."),
            ("return true and ... or ...\n", "return(...)"),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_a_ternary_whose_branches_differ() {
        for source in [
            "return ... and 1 or 2\n",
            "return ... and 1 or 1.0\n",
            "return ... and {1} or {2}\n",
            "return ... and function() end or function(x) end\n",
        ] {
            assert!(remove_dead(source).contains("or"), "source: {source}");
        }
    }

    #[test]
    fn removes_unused_locals() {
        assert_outputs(
            remove_dead,
            &[
                ("local unused = 42 local used = 1 return used", "local used=1 return used"),
                ("local function unused() return 1 end return 2", "return 2"),
                ("local unused = 1 + 2", ""),
                ("local unused = foo()", "foo()"),
                ("local a, b = f(), 1", "f()"),
                ("local a, b = f(), g()", "f()g()"),
            ],
        );
    }

    #[test]
    fn removes_an_unused_local_whose_name_a_used_one_shares() {
        for (source, expected) in [
            ("local x = 1 do local x = f() print(x) end", "do local x=f()print(x)end"),
            ("local k = function(c) local k = c return k end return 1", "return 1"),
            ("local function f(f) return f end return 1", "return 1"),
        ] {
            assert_eq!(walk_once(source, DEAD_CODE), expected, "source: {source}");
        }
    }

    #[test]
    fn removes_a_local_whose_reads_the_walk_drops() {
        for source in [
            "local x = 1 if false then print(x) end",
            "local x = {} local y = function() return x end",
            "local x = {} do local y = function() return x end end",
        ] {
            assert_eq!(walk_once(source, DEAD_CODE), "", "{source}");
        }
    }

    #[test]
    fn uncounts_the_annotation_of_a_name_it_drops() {
        for (source, expected) in [
            ("local a = ... local b: typeof(a) = f()", "f()"),
            ("local a = ... local b: typeof(a), c = 1, g print(c)", "local c=g print(c)"),
        ] {
            assert_eq!(minify_with(source, LuaVersion::Luau, &DEAD_CODE), expected, "{source}");
        }
    }

    #[test]
    fn keeps_unused_env_only_where_it_redirects_globals() {
        let source = "local _ENV = {} return 1";
        assert_eq!(remove_dead(source), "local _ENV={}return 1");
        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            assert_eq!(minify_with(source, version, &DEAD_CODE), "return 1", "{version:?}");
        }
    }

    #[test]
    fn keeps_local_rooting_a_function_declaration() {
        for source in [
            "local M = {} function M.foo() end print(\"ok\")",
            "local M = {} function M:foo() end print(\"ok\")",
            "local M = {} function M.a.b() end print(\"ok\")",
            "local M = {} do function M.foo() end end print(\"ok\")",
            "local M function M() end print(\"ok\")",
        ] {
            assert!(remove_dead(source).contains("local M"), "{source}");
        }
    }

    #[test]
    fn removes_unused_local_named_like_a_function_declaration_field() {
        assert_eq!(remove_dead("local foo = {} function M.foo() end"), "function M.foo()end");
    }

    #[test]
    fn removes_a_run_of_locals_each_used_only_by_the_next_in_one_walk() {
        let mut forward = String::from("local function f0() end\n");
        for i in 1..300 {
            writeln!(forward, "local function f{i}() f{}() end", i - 1).unwrap();
        }
        let mut backward = String::new();
        for i in 0..300 {
            writeln!(backward, "local f{i} = function() f{}() end", i + 1).unwrap();
        }
        for source in [forward, backward] {
            assert_eq!(walk_once(&source, DEAD_CODE), "", "source: {source}");
        }
    }

    #[test]
    fn keeps_a_local_used_by_a_kept_local() {
        let source = "local a = function() end local b = function() return a end return b";
        assert_eq!(
            walk_once(source, DEAD_CODE),
            "local a=function()end local b=function()return a end return b"
        );
    }

    #[test]
    fn removes_an_unused_local_function_that_calls_itself() {
        assert_eq!(walk_once("local function f() return f() end\n", DEAD_CODE), "");
    }

    #[test]
    fn a_kept_call_keeps_what_it_uses() {
        assert_eq!(
            walk_once("local y = 1 local x = function() return y end, g()", DEAD_CODE),
            "g()"
        );
        assert_eq!(walk_once("local y = 1 local x = g(y)", DEAD_CODE), "local y=1 g(y)");
    }

    #[test]
    fn removes_an_unused_local_whose_initializer_reads_only_locals() {
        for source in [
            "local a = ... local b = a return 1",
            "local a = ... local b = {a, not a, a == nil, a and 1} return 1",
            "local a = ... local b = function() return g end return 1",
        ] {
            assert_eq!(remove_dead(source), "return 1", "source: {source}");
        }
    }

    #[test]
    fn drops_a_branch_whose_condition_is_constant_and_reads_only_locals() {
        for (source, expected) in [
            ("local a = ... if a or true then f() end", "do f()end"),
            ("local a = ... if a and nil then f() elseif {a} then g() end", "do g()end"),
            ("local a = ... while a and false do f() end", ""),
            ("local a = ... repeat f() until {a}", "do f()end"),
            ("if g or true then f() end", "if g or true then f()end"),
        ] {
            assert_eq!(remove_dead(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_unused_locals_whose_initializer_can_raise_or_read() {
        for initializer in [
            "g",
            "1 // 0",
            "1 % 0",
            "nil + 1",
            "#nil",
            "\"a\" .. nil",
            "1 < \"a\"",
            "-\"a\"",
            "{[nil] = 1}",
            "{[0/0] = 1}",
        ] {
            let result = remove_dead(&format!("local unused = {initializer} return 1"));
            assert!(result.contains("local unused"), "`{initializer}` must stay: {result}");
        }
    }
}
