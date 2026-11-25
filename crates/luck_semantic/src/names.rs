use std::hash::BuildHasher as _;

use hashbrown::HashTable;
use hashbrown::hash_table::Entry;
use rustc_hash::FxBuildHasher;

use crate::id::NameId;

/// A chunk's distinct names, which it owns, so the tables that use them
/// outlive the tree they came from.
#[derive(Debug, Default)]
pub(crate) struct Names {
    /// Every name's text, one after another in id order.
    text: String,
    entries: Vec<NameEntry>,
    table: HashTable<NameId>,
}

#[derive(Debug, Clone, Copy)]
struct NameEntry {
    start: u32,
    end: u32,
    /// A free name the chunk writes as a global.
    is_assigned_global: bool,
}

/// As bytes, so a comparison checks no char boundaries.
fn entry_bytes<'names>(text: &'names str, entries: &[NameEntry], id: NameId) -> &'names [u8] {
    let entry = entries[id.index()];
    &text.as_bytes()[entry.start as usize..entry.end as usize]
}

fn entry_text<'names>(text: &'names str, entries: &[NameEntry], id: NameId) -> &'names str {
    let entry = entries[id.index()];
    &text[entry.start as usize..entry.end as usize]
}

impl Names {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::with_capacity(capacity * 8),
            entries: Vec::with_capacity(capacity),
            table: HashTable::with_capacity(capacity),
        }
    }

    /// Empties it and keeps its memory, with room for `capacity` names, as
    /// [`Self::with_capacity`] makes.
    pub(crate) fn reset(&mut self, capacity: usize) {
        let Self { text, entries, table } = self;
        text.clear();
        text.reserve(capacity * 8);
        entries.clear();
        entries.reserve(capacity);
        table.clear();
        table.reserve(capacity, |&id| FxBuildHasher.hash_one(entry_text(text, entries, id)));
    }

    pub(crate) const fn len(&self) -> usize {
        self.entries.len()
    }

    /// The id of `name`, which it gets the first time it's asked for.
    pub(crate) fn intern(&mut self, name: &str) -> NameId {
        let Self { text, entries, table } = self;
        let hash = FxBuildHasher.hash_one(name);
        let found = table.entry(
            hash,
            |&id| entry_bytes(text, entries, id) == name.as_bytes(),
            |&id| FxBuildHasher.hash_one(entry_text(text, entries, id)),
        );
        match found {
            Entry::Occupied(occupied) => *occupied.get(),
            Entry::Vacant(vacant) => {
                let id = NameId::from_index(entries.len());
                let start = u32::try_from(text.len())
                    .expect("a chunk's names total fewer bytes than `u32::MAX`");
                text.push_str(name);
                entries.push(NameEntry {
                    start,
                    end: u32::try_from(text.len())
                        .expect("a chunk's names total fewer bytes than `u32::MAX`"),
                    is_assigned_global: false,
                });
                vacant.insert(id);
                id
            }
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<NameId> {
        let hash = FxBuildHasher.hash_one(name);
        self.table
            .find(hash, |&id| entry_bytes(&self.text, &self.entries, id) == name.as_bytes())
            .copied()
    }

    pub(crate) fn text(&self, id: NameId) -> &str {
        entry_text(&self.text, &self.entries, id)
    }

    pub(crate) fn is_assigned_global(&self, id: NameId) -> bool {
        self.entries[id.index()].is_assigned_global
    }

    pub(crate) fn mark_assigned_global(&mut self, id: NameId) {
        self.entries[id.index()].is_assigned_global = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_texts_share_an_id_and_keep_their_text() {
        let mut names = Names::default();
        let alpha = names.intern("alpha");
        let beta = names.intern("beta");
        assert_eq!(names.intern(&String::from("alpha")), alpha);
        assert_ne!(alpha, beta);
        assert_eq!((names.text(alpha), names.text(beta)), ("alpha", "beta"));
        assert_eq!(names.get("beta"), Some(beta));
        assert_eq!(names.get("gamma"), None);
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn a_table_that_grows_finds_every_name() {
        let mut names = Names::default();
        let ids: Vec<NameId> = (0..1_000).map(|i| names.intern(&format!("n{i}"))).collect();
        for (i, id) in ids.into_iter().enumerate() {
            assert_eq!(names.get(&format!("n{i}")), Some(id));
            assert_eq!(names.text(id), format!("n{i}"));
        }
    }
}
