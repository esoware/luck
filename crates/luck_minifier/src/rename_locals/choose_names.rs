//! Choosing each binding's name from what the renamer's walk recorded.

use core::cmp::Reverse;

use super::candidate_pool::CandidatePool;
use super::scope_claims::ScopeClaims;
use super::{BindingKind, Renamer, Scope, UNRESOLVED};
use crate::name_table::NameTable;

/// What choosing names computes, kept for its allocations.
#[derive(Default)]
pub(super) struct Choice<'ast> {
    /// Each name's run of `read_scopes`, the scopes that read it as a
    /// global, ascending.
    read_ranges: Vec<(usize, usize)>,
    read_scopes: Vec<usize>,
    /// `reference_scopes` grouped by binding: binding `b`'s are
    /// `grouped_scopes[group_starts[b]..group_starts[b + 1]]`.
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
    /// Each slot's live scopes, the union of its bindings', as
    /// `slot_scopes[slot_starts[s]..slot_starts[s + 1]]`.
    slot_starts: Vec<usize>,
    slot_scopes: Vec<usize>,
    frequencies: Vec<usize>,
    by_frequency: Vec<usize>,
    slot_names: Vec<&'ast str>,
    /// Each candidate's run of `read_scopes`, for the round being chosen.
    candidate_reads: Vec<Option<(usize, usize)>>,
}

impl Renamer<'_> {
    /// Gives every renamable binding its slot's name and every other one its
    /// own.
    pub(super) fn choose_names(&mut self) {
        let scope_count = self.scopes.len();
        if let Some(root) = self.scopes.first_mut() {
            root.end = scope_count;
        }
        self.group_global_reads();
        self.find_live_scopes();
        let slot_count = self.assign_slots();
        self.name_slots(slot_count);
        let Self { bindings, choice, new_names, .. } = self;
        new_names.clear();
        new_names.extend(bindings.iter().zip(&choice.slot_of).map(|(binding, slot)| {
            slot.and_then(|index| choice.slot_names.get(index).copied()).unwrap_or(binding.name)
        }));
    }

    /// Groups the global reads by name, each name's scopes ascending and
    /// once each.
    fn group_global_reads(&mut self) {
        let Self { global_reads, choice, name_states, .. } = self;
        let ranges = &mut choice.read_ranges;
        ranges.clear();
        ranges.resize(name_states.len(), (0, 0));
        for &(name_id, _) in &*global_reads {
            if let Some(range) = ranges.get_mut(name_id) {
                range.1 = range.1.saturating_add(1);
            }
        }
        // Each range starts where the last ends, and its end moves up from
        // its start as its reads go in.
        let mut total = 0usize;
        for range in ranges.iter_mut() {
            let count = range.1;
            *range = (total, total);
            total = total.saturating_add(count);
        }
        choice.read_scopes.clear();
        choice.read_scopes.resize(total, 0);
        for &(name_id, scope) in &*global_reads {
            if let Some(range) = ranges.get_mut(name_id) {
                if let Some(slot) = choice.read_scopes.get_mut(range.1) {
                    *slot = scope;
                }
                range.1 = range.1.saturating_add(1);
            }
        }
        for range in ranges.iter_mut() {
            let Some(group) = choice.read_scopes.get_mut(range.0..range.1) else {
                continue;
            };
            group.sort_unstable();
            let mut kept = 0usize;
            for index in 0..group.len() {
                let Some(&scope) = group.get(index) else {
                    break;
                };
                if kept == 0 || group.get(kept.saturating_sub(1)) != Some(&scope) {
                    if let Some(slot) = group.get_mut(kept) {
                        *slot = scope;
                    }
                    kept = kept.saturating_add(1);
                }
            }
            range.1 = range.0.saturating_add(kept);
        }
    }

    /// Each renamable binding's live scopes: a scope is live for a binding
    /// on the path from each scope that reads it up to its own.
    fn find_live_scopes(&mut self) {
        let Self { bindings, scopes, reference_scopes, choice, .. } = self;
        let binding_count = bindings.len();
        // Group the reference scopes by binding, keeping walk order.
        let starts = &mut choice.group_starts;
        starts.clear();
        starts.resize(binding_count.saturating_add(1), 0);
        for &(binding, _) in &*reference_scopes {
            if let Some(count) = starts.get_mut(binding) {
                *count = count.saturating_add(1);
            }
        }
        let mut end = 0usize;
        for start in starts.iter_mut() {
            end = end.saturating_add(*start);
            *start = end;
        }
        choice.grouped_scopes.clear();
        choice.grouped_scopes.resize(reference_scopes.len(), 0);
        for &(binding, scope) in reference_scopes.iter().rev() {
            if let Some(start) = starts.get_mut(binding) {
                *start = start.saturating_sub(1);
                if let Some(slot) = choice.grouped_scopes.get_mut(*start) {
                    *slot = scope;
                }
            }
        }
        choice.marks.clear();
        choice.marks.resize(scopes.len(), UNRESOLVED);
        choice.live_starts.clear();
        choice.live_scopes.clear();
        for (id, binding) in bindings.iter().enumerate() {
            choice.live_starts.push(choice.live_scopes.len());
            if binding.kind != BindingKind::Renamable {
                continue;
            }
            if let Some(mark) = choice.marks.get_mut(binding.scope) {
                *mark = id;
            }
            choice.live_scopes.push(binding.scope);
            let group_start = starts.get(id).copied().unwrap_or(0);
            let group_end = starts.get(id.saturating_add(1)).copied().unwrap_or(group_start);
            for &reading in choice.grouped_scopes.get(group_start..group_end).unwrap_or_default() {
                let mut next_scope = Some(reading);
                while let Some(scope) = next_scope
                    && let Some(mark) = choice.marks.get_mut(scope)
                    && *mark != id
                {
                    *mark = id;
                    choice.live_scopes.push(scope);
                    next_scope = scopes.get(scope).and_then(|node| node.parent);
                }
            }
        }
        choice.live_starts.push(choice.live_scopes.len());
    }

    /// Gives each renamable binding the first slot that isn't live in its
    /// scope, in scope preorder, and says how many slots that takes.
    fn assign_slots(&mut self) -> usize {
        let Self { bindings, scopes, choice, .. } = self;
        // Scope ids come from one depth-first walk, so they're in preorder:
        // parents before their children. Within a scope, bindings stay in
        // declaration order.
        let starts = &mut choice.group_starts;
        starts.clear();
        starts.resize(scopes.len(), 0);
        for binding in &*bindings {
            if let Some(count) = starts.get_mut(binding.scope) {
                *count = count.saturating_add(1);
            }
        }
        let mut total = 0usize;
        for start in starts.iter_mut() {
            let count = *start;
            *start = total;
            total = total.saturating_add(count);
        }
        let order = &mut choice.order;
        order.clear();
        order.resize(bindings.len(), 0);
        for (id, binding) in bindings.iter().enumerate() {
            if let Some(start) = starts.get_mut(binding.scope) {
                if let Some(slot) = order.get_mut(*start) {
                    *slot = id;
                }
                *start = start.saturating_add(1);
            }
        }
        choice.slot_of.clear();
        choice.slot_of.resize(bindings.len(), None);
        choice.claims.reset(scopes.len());
        let mut slot_count = 0usize;
        for &id in &*order {
            let Some(binding) = bindings.get(id) else {
                continue;
            };
            if binding.kind != BindingKind::Renamable {
                continue;
            }
            // Reuse the first slot that isn't live in this scope.
            let slot = choice.claims.first_free(binding.scope);
            slot_count = slot_count.max(slot.saturating_add(1));
            let live_start = choice.live_starts.get(id).copied().unwrap_or(0);
            let live_end =
                choice.live_starts.get(id.saturating_add(1)).copied().unwrap_or(live_start);
            choice
                .claims
                .claim(choice.live_scopes.get(live_start..live_end).unwrap_or_default(), slot);
            if let Some(binding_slot) = choice.slot_of.get_mut(id) {
                *binding_slot = Some(slot);
            }
        }
        slot_count
    }

    fn name_slots(&mut self, slot_count: usize) {
        let Self { bindings, scopes, choice, pool, names, .. } = self;
        // Each slot's live scopes, its bindings' in turn.
        choice.slot_starts.clear();
        choice.slot_starts.resize(slot_count.saturating_add(1), 0);
        choice.frequencies.clear();
        choice.frequencies.resize(slot_count, 0);
        for (id, &binding_slot) in choice.slot_of.iter().enumerate() {
            let Some(slot) = binding_slot else {
                continue;
            };
            let live_count = choice
                .live_starts
                .get(id.saturating_add(1))
                .zip(choice.live_starts.get(id))
                .map_or(0, |(end, start)| end.saturating_sub(*start));
            if let Some(count) = choice.slot_starts.get_mut(slot) {
                *count = count.saturating_add(live_count);
            }
            if let (Some(frequency), Some(binding)) =
                (choice.frequencies.get_mut(slot), bindings.get(id))
            {
                *frequency = frequency.saturating_add(binding.references);
            }
        }
        let mut total = 0usize;
        for start in &mut choice.slot_starts {
            total = total.saturating_add(*start);
            *start = total;
        }
        choice.slot_scopes.clear();
        choice.slot_scopes.resize(total, 0);
        for &id in choice.order.iter().rev() {
            let Some(Some(slot)) = choice.slot_of.get(id).copied() else {
                continue;
            };
            let live_start = choice.live_starts.get(id).copied().unwrap_or(0);
            let live_end =
                choice.live_starts.get(id.saturating_add(1)).copied().unwrap_or(live_start);
            for &scope in
                choice.live_scopes.get(live_start..live_end).unwrap_or_default().iter().rev()
            {
                if let Some(start) = choice.slot_starts.get_mut(slot) {
                    *start = start.saturating_sub(1);
                    if let Some(entry) = choice.slot_scopes.get_mut(*start) {
                        *entry = scope;
                    }
                }
            }
        }
        // Most-referenced slots pick first and get the shortest names.
        // Unreferenced slots sort last but still appear in the output, so they
        // go through the same conflict check. Otherwise an unused param could
        // take the name of an upvalue read in the same function body.
        choice.by_frequency.clear();
        choice.by_frequency.extend(0..slot_count);
        let frequencies = &choice.frequencies;
        choice.by_frequency.sort_unstable_by_key(|&slot| {
            (Reverse(frequencies.get(slot).copied().unwrap_or(0)), slot)
        });
        let Choice {
            by_frequency,
            slot_starts,
            slot_scopes,
            claims,
            candidate_reads,
            read_ranges,
            read_scopes,
            slot_names,
            ..
        } = choice;
        claims.reset(scopes.len());
        candidate_reads.clear();
        slot_names.clear();
        slot_names.resize(slot_count, "");
        let reads = Reads { names, ranges: read_ranges, scopes: read_scopes, tree: scopes };
        for &slot in &*by_frequency {
            let start = slot_starts.get(slot).copied().unwrap_or(0);
            let end = slot_starts.get(slot.saturating_add(1)).copied().unwrap_or(start);
            let live_scopes = slot_scopes.get(start..end).unwrap_or_default();
            if let Some(name) = slot_names.get_mut(slot) {
                *name = pick(live_scopes, pool, claims, candidate_reads, &reads);
            }
        }
        for name in slot_names {
            names.intern(name);
        }
    }
}

/// Where each name is read as a global.
struct Reads<'reads, 'ast> {
    names: &'reads NameTable<'ast>,
    ranges: &'reads [(usize, usize)],
    scopes: &'reads [usize],
    tree: &'reads [Scope],
}

impl Reads<'_, '_> {
    fn range_of(&self, name: &str) -> (usize, usize) {
        self.names.find(name).and_then(|id| self.ranges.get(id)).copied().unwrap_or((0, 0))
    }

    /// Whether one of the reads in `range` is in or below `scope`.
    fn is_read_in(&self, range: (usize, usize), scope: usize) -> bool {
        let (start, end) = range;
        let reading_scopes = self.scopes.get(start..end).unwrap_or_default();
        let scope_end = self.tree.get(scope).map_or(scope, |node| node.end);
        let first = reading_scopes.partition_point(|&reading| reading < scope);
        reading_scopes.get(first).is_some_and(|&reading| reading < scope_end)
    }
}

/// The first candidate that no slot live in one of `live_scopes` wears, that
/// isn't a keyword, and that no scope among them reads as a global. Slots
/// that are never live in the same scope can share a name.
fn pick<'ast>(
    live_scopes: &[usize],
    pool: &mut CandidatePool<'ast>,
    claims: &mut ScopeClaims,
    candidate_reads: &mut Vec<Option<(usize, usize)>>,
    reads: &Reads<'_, 'ast>,
) -> &'ast str {
    // Every candidate below a scope's first free one is unavailable there.
    let mut index = live_scopes.iter().map(|&scope| claims.first_free(scope)).max().unwrap_or(0);
    loop {
        let Some((name, is_reserved)) = pool.get(index) else {
            return "";
        };
        // A keyword or a global's name is recorded where it rules the
        // candidate out, so that scope's first free one moves past it.
        if is_reserved {
            claims.claim(live_scopes, index);
        } else {
            if candidate_reads.len() <= index {
                candidate_reads.resize(index.saturating_add(1), None);
            }
            let range = match candidate_reads.get_mut(index) {
                Some(Some(range)) => *range,
                Some(slot) => *slot.insert(reads.range_of(name)),
                None => (0, 0),
            };
            if let Some(scope) = live_scopes
                .iter()
                .copied()
                .find(|&scope| claims.is_claimed(scope, index) || reads.is_read_in(range, scope))
            {
                claims.claim_in(scope, index);
            } else {
                claims.claim(live_scopes, index);
                return name;
            }
        }
        index = index.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_allocator::Allocator;
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::{RENAMING, minify_with};

    fn apply(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &RENAMING)
    }

    /// Picks a name for each slot in turn, in ten sibling scopes under the
    /// root, where `global_reads` reads each name in a scope.
    fn pick_names<'ast>(
        pool: &mut CandidatePool<'ast>,
        claims: &mut ScopeClaims,
        slots: &[&[usize]],
        global_reads: &[(&'ast str, usize)],
    ) -> Vec<&'ast str> {
        let tree: Vec<Scope> = (0..10usize)
            .map(|id| Scope {
                parent: (id > 0).then_some(0),
                end: if id == 0 { 10 } else { id.saturating_add(1) },
            })
            .collect();
        let mut names = NameTable::default();
        let mut ranges = Vec::new();
        let mut scopes = Vec::new();
        for &(name, scope) in global_reads {
            assert_eq!(names.intern_text(name), ranges.len());
            ranges.push((scopes.len(), scopes.len().saturating_add(1)));
            scopes.push(scope);
        }
        let reads = Reads { names: &names, ranges: &ranges, scopes: &scopes, tree: &tree };
        let mut candidate_reads = Vec::new();
        slots
            .iter()
            .map(|live_scopes| pick(live_scopes, pool, claims, &mut candidate_reads, &reads))
            .collect()
    }

    #[test]
    fn slots_that_share_no_scope_share_a_name() {
        let allocator = Allocator::new();
        let mut pool = CandidatePool::new(&allocator, LuaVersion::Lua54);
        let mut claims = ScopeClaims::default();
        claims.reset(10);
        let slots: [&[usize]; 5] = [&[0, 1], &[1, 2], &[2, 3], &[4], &[0, 4]];
        assert_eq!(pick_names(&mut pool, &mut claims, &slots, &[]), ["l", "u", "l", "l", "u"]);
    }

    #[test]
    fn pick_skips_a_global_read_in_any_live_scope() {
        let allocator = Allocator::new();
        let mut pool = CandidatePool::new(&allocator, LuaVersion::Lua54);
        let mut claims = ScopeClaims::default();
        claims.reset(10);
        let slots: [&[usize]; 3] = [&[1, 3], &[3], &[1]];
        let reads = [("l", 3), ("c", 3)];
        assert_eq!(pick_names(&mut pool, &mut claims, &slots, &reads), ["u", "k", "l"]);
    }

    #[test]
    fn pick_skips_reserved_words() {
        // Every one- and two-character name, so each two-letter keyword
        // comes up.
        let pick_count = 53 * 64;
        for version in LuaVersion::ALL {
            let allocator = Allocator::new();
            let mut pool = CandidatePool::new(&allocator, version);
            let mut claims = ScopeClaims::default();
            claims.reset(10);
            let slots = vec![&[0usize][..]; pick_count];
            let names = pick_names(&mut pool, &mut claims, &slots, &[]);
            let distinct: rustc_hash::FxHashSet<&str> = names.iter().copied().collect();
            assert_eq!(distinct.len(), pick_count, "{version:?}");
            for keyword in ["do", "if", "in", "or"] {
                assert!(!distinct.contains(keyword), "{keyword} {version:?}");
            }
        }
    }

    #[test]
    fn a_pick_among_many_live_slots_starts_past_the_names_they_wear() {
        let allocator = Allocator::new();
        let mut pool = CandidatePool::new(&allocator, LuaVersion::Lua54);
        let mut claims = ScopeClaims::default();
        claims.reset(10);
        pick_names(&mut pool, &mut claims, &vec![&[0usize][..]; 5_000], &[]);
        // Without each scope's first free candidate, every pick rescans the
        // names before it.
        assert!(claims.first_free(0) >= 5_000);
    }

    #[test]
    fn many_locals_live_at_once_rename_without_rescanning_slots_or_names() {
        // Each local needs a slot and a name of its own. Looking either up
        // from the first one every time takes minutes at this size.
        let mut source = String::new();
        for local in 0..120_000u32 {
            let _unused = writeln!(source, "local a{local} = f({local})");
        }
        source.push_str("return a0, a119999");
        let result = apply(&source);
        // The two locals read twice take the two shortest names.
        assert!(
            result.starts_with("local l=f(0)local c=f(1)local k=f(2)"),
            "{:?}",
            result.get(..60)
        );
        assert!(result.ends_with("return l,u"), "the output ends in the return");
    }

    #[test]
    fn many_sibling_scopes_share_one_name() {
        let mut source = String::new();
        for scope in 0..20_000u32 {
            let _unused = writeln!(source, "do local x{scope} = f() g(x{scope}) end");
        }
        let result = apply(&source);
        assert_eq!(result, "do local l=f()g(l)end ".repeat(20_000).trim_end());
    }
}
