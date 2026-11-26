use std::iter::Map;
use std::slice;

use luck_allocator::Allocator;

/// Library entries by name, in name order, with a hash table over them, in
/// the library's arena.
#[derive(Debug, Clone, Copy)]
pub struct StdlibMap<'lib, V> {
    entries: &'lib [(&'lib str, V)],
    /// Open addressing by [`name_hash`]: each slot holds an entry's index plus
    /// one, or zero when it's empty, and at least half of them are.
    slots: &'lib [u32],
}

type Entries<'lib, V> =
    Map<slice::Iter<'lib, (&'lib str, V)>, fn(&'lib (&'lib str, V)) -> (&'lib str, &'lib V)>;

/// FNV-1a.
fn name_hash(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in name.as_bytes() {
        hash = (hash ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    hash
}

impl<'lib, V> StdlibMap<'lib, V> {
    pub(crate) const EMPTY: Self = Self { entries: &[], slots: &[] };

    /// The map of `sorted`, sorted by name with no name twice, in
    /// `allocator`.
    pub(crate) fn new_in(allocator: &'lib Allocator, sorted: &[(&'lib str, V)]) -> Self
    where
        V: Copy,
    {
        let entries = allocator.alloc_slice_copy(sorted);
        let mut slots = vec![0u32; (entries.len() * 2).next_power_of_two()];
        let mask = slots.len() - 1;
        for (index, (name, _)) in (1..).zip(entries.iter()) {
            let mut slot = name_hash(name) as usize & mask;
            while slots[slot] != 0 {
                slot = (slot + 1) & mask;
            }
            slots[slot] = index;
        }
        Self { entries, slots: allocator.alloc_slice_copy(&slots) }
    }

    pub fn get(&self, name: &str) -> Option<&'lib V> {
        let mask = self.slots.len().checked_sub(1)?;
        let mut slot = name_hash(name) as usize & mask;
        // An empty slot ends every probe, since at least half are empty.
        loop {
            let entry = self.slots[slot].checked_sub(1)?;
            let (entry_name, value) = &self.entries[entry as usize];
            if *entry_name == name {
                return Some(value);
            }
            slot = (slot + 1) & mask;
        }
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn iter(&self) -> Entries<'lib, V> {
        let pair: fn(&'lib (&'lib str, V)) -> (&'lib str, &'lib V) = |(name, value)| (*name, value);
        self.entries.iter().map(pair)
    }

    pub fn keys(&self) -> impl Iterator<Item = &'lib str> {
        self.entries.iter().map(|(name, _)| *name)
    }

    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<'lib, V> IntoIterator for &StdlibMap<'lib, V> {
    type Item = (&'lib str, &'lib V);
    type IntoIter = Entries<'lib, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_finds_exactly_its_names() {
        let allocator = Allocator::new();
        let map = StdlibMap::new_in(&allocator, &[("b", 1u8), ("d", 2), ("f", 3)]);
        assert_eq!(map.get("d"), Some(&2));
        for absent in ["", "a", "c", "e", "g"] {
            assert_eq!(map.get(absent), None, "{absent}");
        }
        assert_eq!(map.iter().collect::<Vec<_>>(), [("b", &1), ("d", &2), ("f", &3)]);
        let empty: StdlibMap<'_, u8> = StdlibMap::EMPTY;
        assert_eq!(empty.get("b"), None);
    }
}
