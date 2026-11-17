//! Dropping the locals of a block that nothing references, as a walk leaves
//! the block.
//!
//! `remove_dead_code` does it in two walks: the bind walk drops the locals
//! nothing references from the start, and the compressor those its rewrites
//! leave unreferenced. A local is referenced only in its block, so once a
//! walk leaves the block, every reference that's left is counted. The
//! block's statements go last first, since each one's initializer can hold
//! the last reference to one before it, and every reference in what goes is
//! uncounted.

use core::mem;

use luck_allocator::{Allocator, List};
use luck_ast::{Expression, LocalAssignment, Statement, Visitor as _};
use luck_token::LuaVersion;

use crate::binding::{
    Bindings, Counter, has_attributed_or_env_name, has_fixed_binding, is_droppable_function,
    local_of,
};
use crate::expr::is_pure_expr;

/// What becomes of a `local` when none of its names is used.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnusedLocalAction {
    Keep,
    Remove,
    /// Every initializer is pure or a call, so only the calls stay.
    ExtractCalls,
}

/// The rewrites can't make a value less pure, so a `local` this finds
/// droppable before a walk is droppable after it.
fn unused_local_action(local: &LocalAssignment<'_>, version: LuaVersion) -> UnusedLocalAction {
    // Exports, attributed names, and a 5.2+ `_ENV` are never dead.
    if local.kind.is_exported() || has_attributed_or_env_name(local, version) {
        return UnusedLocalAction::Keep;
    }
    if local.exprs.iter().all(is_pure_expr) {
        UnusedLocalAction::Remove
    } else if local.exprs.iter().all(|expr| is_pure_expr(expr) || is_call(expr)) {
        UnusedLocalAction::ExtractCalls
    } else {
        UnusedLocalAction::Keep
    }
}

fn is_call(expr: &Expression<'_>) -> bool {
    matches!(expr, Expression::Suffixed(chain) if chain.is_call())
}

/// A walk's view of the block it's leaving.
pub(crate) struct UnusedLocals<'walk, 'ast> {
    pub allocator: &'ast Allocator,
    pub version: LuaVersion,
    pub bindings: &'walk mut Bindings,
    /// Where the walk is, as [`Counter`] counts it.
    pub function_depth: u32,
    pub type_floor: u32,
    /// The calls each unused `local` leaves past the first, by its index.
    pub calls: &'walk mut Vec<(usize, Statement<'ast>)>,
}

impl<'ast> UnusedLocals<'_, 'ast> {
    /// Drops what of `stmts` nothing references, and says whether it
    /// dropped anything.
    pub(crate) fn drop_unused(&mut self, stmts: &mut List<'ast, Statement<'ast>>) -> bool {
        let mut has_dropped = false;
        for (index, stmt) in stmts.iter_mut().enumerate().rev() {
            match self.drop_unused_from(stmt, index) {
                Dropped::Nothing => {}
                Dropped::Part => has_dropped = true,
                Dropped::Whole => {
                    *stmt = Statement::EmptyStatement(stmt.span());
                    has_dropped = true;
                }
            }
        }
        if !has_dropped {
            return false;
        }
        let mut calls = self.calls.drain(..).rev().peekable();
        let mut kept = Vec::with_capacity(stmts.len());
        for (index, stmt) in mem::take(stmts).into_iter().enumerate() {
            if !matches!(stmt, Statement::EmptyStatement(_)) {
                kept.push(stmt);
            }
            while let Some((_, call)) = calls.next_if(|&(call_index, _)| call_index == index) {
                kept.push(call);
            }
        }
        *stmts = List::from_iter_in(kept, self.allocator);
        true
    }

    fn is_used(&self, name: &luck_ast::Name<'_>) -> bool {
        local_of(name).is_some_and(|id| self.bindings.is_used(id))
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

    /// Drops what of the statement at `index` nothing references.
    fn drop_unused_from(&mut self, stmt: &mut Statement<'ast>, index: usize) -> Dropped {
        match stmt {
            Statement::LocalFunction(local_function) => {
                if is_droppable_function(local_function, self.version)
                    && !self.is_used(&local_function.name)
                {
                    self.counter().visit_stmt(stmt);
                    return Dropped::Whole;
                }
                Dropped::Nothing
            }
            Statement::LocalAssignment(local) => {
                if local.names.iter().all(|attributed| !self.is_used(&attributed.name)) {
                    match unused_local_action(local, self.version) {
                        UnusedLocalAction::Keep => {}
                        UnusedLocalAction::Remove => {
                            self.counter().visit_stmt(stmt);
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
                let mut kept = 0usize;
                for i in 0..local.names.len() {
                    let (Some(attributed), Some(value)) = (local.names.get(i), local.exprs.get(i))
                    else {
                        break;
                    };
                    if !self.is_used(&attributed.name) && is_pure_expr(value) {
                        self.counter().visit_expr(value);
                    } else {
                        local.names.swap(kept, i);
                        local.exprs.swap(kept, i);
                        kept = kept.saturating_add(1);
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
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
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
        let mut first_call = None;
        let calls_start = self.calls.len();
        for expr in &mut local.exprs {
            if !is_call(expr) {
                self.counter().visit_expr(expr);
                continue;
            }
            if let Expression::Suffixed(chain) = expr.take() {
                let call = Statement::FunctionCall(chain);
                if first_call.is_none() {
                    first_call = Some(call);
                } else {
                    self.calls.push((index, call));
                }
            }
        }
        // The calls go in last first, as the block's statements do.
        if let Some(later) = self.calls.get_mut(calls_start..) {
            later.reverse();
        }
        if let Some(call) = first_call {
            *stmt = call;
        }
    }
}

enum Dropped {
    Nothing,
    Part,
    Whole,
}
