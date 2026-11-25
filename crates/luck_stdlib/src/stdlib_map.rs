use std::iter::Map;
use std::slice;

use luck_allocator::Allocator;

/// Library entries by name, in name order, with a hash table over them,
/// built at compile time for a built-in library and in its arena for a
/// layered one.
#[derive(Debug, Clone, Copy)]
pub struct StdlibMap<'lib, V> {
    entries: &'lib [(&'lib str, V)],
    /// Open addressing by [`name_hash`]: each slot holds an entry's index plus
    /// one, or zero when it's empty, and at least half of them are.
    slots: &'lib [u16],
}

type Entries<'lib, V> =
    Map<slice::Iter<'lib, (&'lib str, V)>, fn(&'lib (&'lib str, V)) -> (&'lib str, &'lib V)>;

/// A static [`StdlibMap`] of `$value`s from `(name, value)` entries sorted by
/// name, with no name twice.
///
/// The entries are a `static`, since a value may point to another `static`,
/// as a constant set does.
macro_rules! stdlib_map {
    ($value:ty, [$($entry:expr),* $(,)?]) => {{
        const COUNT: usize = <[()]>::len(&[$($crate::stdlib_map::stdlib_map!(@unit $entry)),*]);
        static ENTRIES: [(&str, $value); COUNT] = [$($entry),*];
        static SLOTS: [u16; $crate::stdlib_map::slot_count(COUNT)] =
            $crate::stdlib_map::hash_slots(&ENTRIES);
        $crate::stdlib_map::StdlibMap::new(&ENTRIES, &SLOTS)
    }};
    (@unit $entry:expr) => {
        ()
    };
}

pub(crate) use stdlib_map;

/// The most entries a [`StdlibMap`] holds, since a `u16` slot holds an
/// entry's index plus one.
pub(crate) const MAX_MAP_ENTRIES: usize = 0xFFFF;

/// FNV-1a, which both compile-time table building and lookups run.
const fn name_hash(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    let mut remaining = name.as_bytes();
    while let [b, rest @ ..] = remaining {
        hash = (hash ^ *b as u32).wrapping_mul(0x0100_0193);
        remaining = rest;
    }
    hash
}

pub(crate) const fn slot_count(entry_count: usize) -> usize {
    (entry_count * 2).next_power_of_two()
}

pub(crate) const fn hash_slots<V, const SLOTS: usize>(entries: &[(&str, V)]) -> [u16; SLOTS] {
    let mut slots = [0; SLOTS];
    fill_slots(entries, &mut slots);
    slots
}

/// Fills `slots`, [`slot_count`] of them, all zero, with the hash table over
/// `entries`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the assert keeps `index` within a `u16`, and `u16::try_from` isn't const"
)]
const fn fill_slots<V>(entries: &[(&str, V)], slots: &mut [u16]) {
    assert!(entries.len() <= MAX_MAP_ENTRIES, "a stdlib table has too many entries for its slots");
    let mask = slots.len() - 1;
    let mut index = 0usize;
    let mut remaining = entries;
    while let [(name, _), rest @ ..] = remaining {
        index += 1;
        let mut slot = name_hash(name) as usize & mask;
        while slots[slot] != 0 {
            slot = (slot + 1) & mask;
        }
        slots[slot] = index as u16;
        remaining = rest;
    }
}

impl<'lib, V> StdlibMap<'lib, V> {
    pub(crate) const EMPTY: Self = Self { entries: &[], slots: &[] };

    /// `slots` is [`hash_slots`] of `entries`, which [`stdlib_map`] builds.
    pub(crate) const fn new(entries: &'lib [(&'lib str, V)], slots: &'lib [u16]) -> Self {
        Self { entries, slots }
    }

    /// The map of `entries`, sorted by name with no name twice and at most
    /// [`MAX_MAP_ENTRIES`] of them, in `allocator`.
    pub(crate) fn new_in(allocator: &'lib Allocator, sorted: &[(&'lib str, V)]) -> Self
    where
        V: Copy,
    {
        let entries = allocator.alloc_slice_copy(sorted);
        let mut slots = vec![0; slot_count(entries.len())];
        fill_slots(entries, &mut slots);
        Self { entries, slots: allocator.alloc_slice_copy(&slots) }
    }

    pub fn get(&self, name: &str) -> Option<&'lib V> {
        let mask = self.slots.len().checked_sub(1)?;
        let mut slot = name_hash(name) as usize & mask;
        // An empty slot ends every probe, since at least half are empty.
        loop {
            let entry = self.slots[slot].checked_sub(1)?;
            let (entry_name, value) = &self.entries[usize::from(entry)];
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
        static MAP: StdlibMap<'static, u8> = stdlib_map!(u8, [("b", 1), ("d", 2), ("f", 3)]);
        assert_eq!(MAP.get("d"), Some(&2));
        for absent in ["", "a", "c", "e", "g"] {
            assert_eq!(MAP.get(absent), None, "{absent}");
        }
        assert_eq!(MAP.iter().collect::<Vec<_>>(), [("b", &1), ("d", &2), ("f", &3)]);
    }

    #[test]
    fn a_map_built_in_an_arena_finds_exactly_its_names() {
        let allocator = Allocator::new();
        let map = StdlibMap::new_in(&allocator, &[("b", 1u8), ("d", 2), ("f", 3)]);
        assert_eq!(map.get("f"), Some(&3));
        assert_eq!(map.get("c"), None);
        let empty: StdlibMap<'_, u8> = StdlibMap::EMPTY;
        assert_eq!(empty.get("b"), None);
    }
}
