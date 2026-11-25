//! Choosing each binding's name from what the renamer read of the scoping,
//! as `oxc_mangler` chooses them.

use std::cmp::Reverse;

use luck_semantic::NameId;

use super::candidate_pool::CandidatePool;
use super::scope_claims::ScopeClaims;
use super::{Renamer, Scope, UNRESOLVED};

/// What the steps of choosing names compute, each step from what the ones
/// before it left.
#[derive(Default)]
struct Choice {
    /// Each name's run of `read_scopes`, the scopes that read it as a
    /// global, ascending.
    read_ranges: Vec<(usize, usize)>,
    read_scopes: Vec<usize>,
    /// `reference_scopes` grouped by binding: binding `b`'s are
    /// `grouped_scopes[group_starts[b]..group_starts[b + 1]]`. Once the live
    /// scopes are found, slots reuse `group_starts` to put each scope's
    /// bindings in `order`.
    group_starts: Vec<usize>,
    grouped_scopes: Vec<usize>,
    /// Each renamable binding's live scopes: its own, its reference scopes,
    /// and every scope between them, as `live_scopes[live_starts[b]..live_starts[b + 1]]`.
    live_starts: Vec<usize>,
    live_scopes: Vec<usize>,
    /// The binding that last marked each scope live.
    marks: Vec<usize>,
    /// The bindings in scope preorder, each scope's in declaration order.
    order: Vec<usize>,
    slot_of: Vec<Option<usize>>,
    claims: ScopeClaims,
    /// The scopes each slot's bindings are declared in, ascending, as
    /// `declared_scopes[declared_starts[s]..declared_starts[s + 1]]`.
    declared_starts: Vec<usize>,
    declared_scopes: Vec<usize>,
    /// How many names spell each slot's bindings.
    occurrences: Vec<usize>,
    by_occurrences: Vec<usize>,
    /// The candidate each slot wears.
    slot_candidates: Vec<usize>,
}

impl Renamer<'_> {
    /// The name renaming gives each binding: its slot's, or none for one
    /// that keeps its own.
    pub(super) fn choose_names<'ast>(
        &self,
        pool: &mut CandidatePool<'_, 'ast>,
    ) -> Vec<Option<&'ast str>> {
        let mut choice = Choice::default();
        self.group_global_reads(&mut choice);
        self.find_live_scopes(&mut choice);
        let slot_count = self.assign_slots(&mut choice);
        self.name_slots(&mut choice, slot_count, pool);
        choice
            .slot_of
            .iter()
            .map(|binding_slot| {
                binding_slot.map(|slot| pool.get(choice.slot_candidates[slot]).name)
            })
            .collect()
    }

    /// Groups the global reads by name, each name's scopes ascending and
    /// once each.
    fn group_global_reads(&self, choice: &mut Choice) {
        let Self { global_reads, scoping, .. } = self;
        let ranges = &mut choice.read_ranges;
        ranges.resize(scoping.names().len(), (0, 0));
        for &(name_id, _) in global_reads {
            ranges[name_id].1 += 1;
        }
        // Each range starts where the last ends, and its end moves up from
        // its start as its reads go in.
        let mut total = 0;
        for range in ranges.iter_mut() {
            let count = range.1;
            *range = (total, total);
            total += count;
        }
        choice.read_scopes.resize(total, 0);
        for &(name_id, scope) in global_reads {
            let range = &mut ranges[name_id];
            choice.read_scopes[range.1] = scope;
            range.1 += 1;
        }
        for range in ranges.iter_mut() {
            let group = &mut choice.read_scopes[range.0..range.1];
            group.sort_unstable();
            let mut kept = 0;
            for index in 0..group.len() {
                let scope = group[index];
                if kept == 0 || group[kept - 1] != scope {
                    group[kept] = scope;
                    kept += 1;
                }
            }
            range.1 = range.0 + kept;
        }
    }

    /// Each renamable binding's live scopes: a scope is live for a binding
    /// on the path from each scope that reads it up to its own.
    fn find_live_scopes(&self, choice: &mut Choice) {
        let Self { renamed: bindings, scopes, reference_scopes, .. } = self;
        let binding_count = bindings.len();
        // Group the reference scopes by binding, keeping walk order.
        let starts = &mut choice.group_starts;
        starts.resize(binding_count + 1, 0);
        for &(binding, _) in reference_scopes {
            starts[binding] += 1;
        }
        let mut end = 0;
        for start in starts.iter_mut() {
            end += *start;
            *start = end;
        }
        choice.grouped_scopes.resize(reference_scopes.len(), 0);
        for &(binding, scope) in reference_scopes.iter().rev() {
            let start = &mut starts[binding];
            *start -= 1;
            choice.grouped_scopes[*start] = scope;
        }
        choice.marks.resize(scopes.len(), UNRESOLVED);
        for (id, binding) in bindings.iter().enumerate() {
            choice.live_starts.push(choice.live_scopes.len());
            if binding.is_fixed {
                continue;
            }
            choice.marks[binding.scope] = id;
            choice.live_scopes.push(binding.scope);
            for &reading in &choice.grouped_scopes[starts[id]..starts[id + 1]] {
                let mut next_scope = Some(reading);
                while let Some(scope) = next_scope
                    && choice.marks[scope] != id
                {
                    choice.marks[scope] = id;
                    choice.live_scopes.push(scope);
                    next_scope = scopes[scope].parent;
                }
            }
        }
        choice.live_starts.push(choice.live_scopes.len());
    }

    /// Gives each renamable binding the first slot that isn't live in its
    /// scope, in scope preorder, and says how many slots that takes.
    ///
    /// A new slot opens only where every slot before it is live, so every
    /// two slots are live in some scope together, and no two can share a
    /// name.
    fn assign_slots(&self, choice: &mut Choice) -> usize {
        let Self { renamed: bindings, scopes, .. } = self;
        // Scope ids come from one depth-first walk, so they're in preorder:
        // parents before their children. Within a scope, bindings stay in
        // declaration order.
        let starts = &mut choice.group_starts;
        starts.clear();
        starts.resize(scopes.len(), 0);
        for binding in bindings {
            starts[binding.scope] += 1;
        }
        let mut total = 0;
        for start in starts.iter_mut() {
            let count = *start;
            *start = total;
            total += count;
        }
        let order = &mut choice.order;
        order.resize(bindings.len(), 0);
        for (id, binding) in bindings.iter().enumerate() {
            let start = &mut starts[binding.scope];
            order[*start] = id;
            *start += 1;
        }
        choice.slot_of.resize(bindings.len(), None);
        choice.claims.reset(scopes.len());
        let mut slot_count = 0;
        for &id in &*order {
            let binding = &bindings[id];
            if binding.is_fixed {
                continue;
            }
            // Reuse the first slot that isn't live in this scope.
            let slot = choice.claims.first_free(binding.scope);
            slot_count = slot_count.max(slot + 1);
            let live_scopes =
                &choice.live_scopes[choice.live_starts[id]..choice.live_starts[id + 1]];
            choice.claims.claim(live_scopes, slot);
            choice.slot_of[id] = Some(slot);
        }
        slot_count
    }

    /// Names the slots, the most often spelled first, each the first
    /// candidate it can wear.
    ///
    /// `oxc_mangler` then hands each length's names out again in slot order,
    /// after Closure Compiler's `RenameVars`, which compresses worse on Luau.
    fn name_slots(&self, choice: &mut Choice, slot_count: usize, pool: &mut CandidatePool<'_, '_>) {
        let Self { renamed: bindings, scopes, .. } = self;
        let starts = &mut choice.declared_starts;
        starts.resize(slot_count + 1, 0);
        choice.occurrences.resize(slot_count, 0);
        for (binding, &binding_slot) in bindings.iter().zip(&choice.slot_of) {
            let Some(slot) = binding_slot else {
                continue;
            };
            starts[slot] += 1;
            choice.occurrences[slot] += binding.occurrences;
        }
        let mut total = 0;
        for start in starts.iter_mut() {
            total += *start;
            *start = total;
        }
        choice.declared_scopes.resize(total, 0);
        for &id in choice.order.iter().rev() {
            if let Some(slot) = choice.slot_of[id] {
                let start = &mut starts[slot];
                *start -= 1;
                choice.declared_scopes[*start] = bindings[id].scope;
            }
        }
        let reads =
            Reads { ranges: &choice.read_ranges, scopes: &choice.read_scopes, tree: scopes };
        let by_occurrences = &mut choice.by_occurrences;
        by_occurrences.extend(0..slot_count);
        let occurrences = &choice.occurrences;
        by_occurrences.sort_unstable_by_key(|&slot| (Reverse(occurrences[slot]), slot));
        choice.slot_candidates.resize(slot_count, 0);
        let mut picker = Picker::default();
        for &slot in &*by_occurrences {
            let declared_scopes = &choice.declared_scopes[starts[slot]..starts[slot + 1]];
            choice.slot_candidates[slot] = picker.pick(declared_scopes, pool, &reads);
        }
    }
}

/// Where each name is read as a global, by its [`NameId`].
struct Reads<'reads> {
    ranges: &'reads [(usize, usize)],
    scopes: &'reads [usize],
    tree: &'reads [Scope],
}

impl Reads<'_> {
    /// Whether the chunk reads `name` as a global in or below one of
    /// `scopes`, where a binding named so would capture the read.
    fn is_read_below(&self, name: Option<NameId>, scopes: &[usize]) -> bool {
        let Some(name) = name else {
            return false;
        };
        let (start, end) = self.ranges[name.index()];
        let reading_scopes = &self.scopes[start..end];
        !reading_scopes.is_empty()
            && scopes.iter().any(|&scope| {
                let scope_end = self.tree[scope].end;
                let first = reading_scopes.partition_point(|&reading| reading < scope);
                reading_scopes.get(first).is_some_and(|&reading| reading < scope_end)
            })
    }
}

/// Gives each slot that asks the first candidate it can wear: no keyword,
/// no other slot's, and none the chunk reads as a global in or below a
/// scope one of the slot's bindings is declared in.
#[derive(Default)]
struct Picker {
    /// Every candidate below it is a keyword, another slot's, or passed over.
    next: usize,
    /// The candidates below `next` a slot passed over for a global it would
    /// capture, which a later slot may still wear, ascending.
    passed_over: Vec<(usize, Option<NameId>)>,
}

impl Picker {
    fn pick(
        &mut self,
        declared_scopes: &[usize],
        pool: &mut CandidatePool<'_, '_>,
        reads: &Reads<'_>,
    ) -> usize {
        if let Some(position) = self
            .passed_over
            .iter()
            .position(|&(_, name)| !reads.is_read_below(name, declared_scopes))
        {
            return self.passed_over.remove(position).0;
        }
        loop {
            let index = self.next;
            self.next = index + 1;
            let candidate = pool.get(index);
            if candidate.is_reserved {
                continue;
            }
            if reads.is_read_below(candidate.name_id, declared_scopes) {
                self.passed_over.push((index, candidate.name_id));
                continue;
            }
            return index;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use crate::fixtures::{RENAMING, minify_with};

    fn apply(source: &str) -> String {
        minify_with(source, &RENAMING)
    }

    #[test]
    fn the_slot_spelled_most_picks_first_declarations_included() {
        // `a` is read three times, and the slot of `x`, `y`, and `z` three
        // times too, but its three declarations spell it more.
        let result = apply(
            "\
local a = f()
do local x = a print(x) end
do local y = a print(y) end
do local z = a print(z) end
",
        );
        assert_eq!(
            result,
            "local t=f()do local e=t print(e)end do local e=t print(e)end do local e=t print(e)end"
        );
    }

    #[test]
    fn a_name_passed_over_for_a_global_read_goes_to_a_later_slot() {
        // The hotter slot can't wear `e`, which its first block reads, but
        // the second block's other local can.
        let result = apply(
            "\
do
  local x = f()
  print(x, x, x, e)
end
do
  local y = f()
  local z = f()
  print(y, z)
end
",
        );
        assert_eq!(
            result,
            "do local t=f()print(t,t,t,e)end do local t=f()local e=f()print(t,e)end"
        );
    }

    #[test]
    fn many_locals_live_at_once_rename_without_rescanning_slots_or_names() {
        // Each local needs a slot and a name of its own. Looking either up
        // from the first one every time takes minutes at this size.
        let mut source = String::new();
        for local in 0..120_000 {
            writeln!(source, "local a{local} = f({local})").unwrap();
        }
        source.push_str("return a0, a119999");
        let result = apply(&source);
        // The two locals read twice take the first two names.
        assert!(
            result.starts_with("local e=f(0)local n=f(1)local a=f(2)"),
            "{:?}",
            result.get(..60)
        );
        assert!(result.ends_with("return e,t"), "the output ends in the return");
    }

    #[test]
    fn many_sibling_scopes_share_one_name() {
        let mut source = String::new();
        for scope in 0..20_000 {
            writeln!(source, "do local x{scope} = f() g(x{scope}) end").unwrap();
        }
        let result = apply(&source);
        assert_eq!(result, "do local e=f()g(e)end ".repeat(20_000).trim_end());
    }
}
