//! A `local` statement's walk, which `remove_dead_code` and `inline_locals`
//! share.
//!
//! Both decide each of its names by what the scan found of its uses: a name
//! nothing kept uses goes, and one read once moves to its read.

use core::mem;

use luck_allocator::List;
use luck_ast::{Statement, VisitorMut as _};

use super::Compressor;
use super::remove_dead_code::is_nil_initialized;
use crate::binding::has_fixed_binding;
use crate::expr::is_pure_expr;
use crate::scan::{DeadLocalAction, unused_local_action};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the compressor is one type with its rewrites split by flag"
)]
impl<'ast> Compressor<'ast> {
    /// Walks a `local` and drops what of it is dead or inlines: the whole
    /// statement, or in one that gives each name its own value, a name and
    /// its value. Says whether anything of it stays.
    pub(super) fn visit_local(&mut self, stmt: &mut Statement<'ast>) -> bool {
        // An initializer can hold `local`s of its own, so each list is a
        // stack that every `local` leaves as it found it.
        let facts_start = self.local_facts.len();
        let landed_start = self.landed_counts.len();
        let is_kept = self.visit_local_from(stmt, facts_start, landed_start);
        self.local_facts.truncate(facts_start);
        self.landed_counts.truncate(landed_start);
        is_kept
    }

    fn visit_local_from(
        &mut self,
        stmt: &mut Statement<'ast>,
        facts_start: usize,
        landed_start: usize,
    ) -> bool {
        let Statement::LocalAssignment(boxed) = stmt else {
            return true;
        };
        let local = &mut **boxed;
        let should_remove_dead = self.config.remove_dead_code;
        let Self { scan, names, local_facts, .. } = self;
        local_facts.extend(
            local.names.iter().map(|attributed| scan.local(names.find(attributed.name.text))),
        );
        let is_dead = should_remove_dead
            && !self
                .local_facts
                .get(facts_start..)
                .unwrap_or_default()
                .iter()
                .any(|&(is_live, _)| is_live);
        if is_dead && unused_local_action(local, self.version) == DeadLocalAction::Remove {
            return false;
        }
        self.walk_attributed_names(&mut local.names);
        for expr in &mut local.exprs {
            let outer_landed_locals = mem::replace(&mut self.landed_locals, 0);
            self.visit_expr(expr);
            let landed_locals = mem::replace(&mut self.landed_locals, outer_landed_locals);
            self.landed_counts.push(landed_locals);
        }
        if is_dead {
            match unused_local_action(local, self.version) {
                DeadLocalAction::Keep => {}
                DeadLocalAction::Remove => return false,
                DeadLocalAction::ExtractCalls => {
                    self.extract_calls(stmt);
                    return true;
                }
            }
        }
        if should_remove_dead && is_nil_initialized(local) {
            local.exprs = List::new();
            self.has_changed = true;
            return true;
        }
        // Dropping a name and its value keeps every other name's value only
        // when each name has its own.
        if has_fixed_binding(local, self.version) || local.names.len() != local.exprs.len() {
            return true;
        }
        let mut kept = 0usize;
        for index in 0..local.names.len() {
            let (
                Some(attributed),
                Some(value),
                Some(&landed_locals),
                Some(&(is_live, inline_read_depth)),
            ) = (
                local.names.get(index),
                local.exprs.get_mut(index),
                self.landed_counts.get(landed_start.saturating_add(index)),
                self.local_facts.get(facts_start.saturating_add(index)),
            )
            else {
                break;
            };
            let name = attributed.name.text;
            let is_dropped = (should_remove_dead && !is_live && is_pure_expr(value))
                || (self.config.inline_locals
                    && self.hold_value(name, value, landed_locals, inline_read_depth));
            if is_dropped {
                self.has_changed = true;
            } else {
                local.names.swap(kept, index);
                local.exprs.swap(kept, index);
                kept = kept.saturating_add(1);
            }
        }
        local.names.truncate(kept);
        local.exprs.truncate(kept);
        kept > 0
    }
}
