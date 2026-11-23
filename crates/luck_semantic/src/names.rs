use core::hash::BuildHasher as _;

use hashbrown::HashTable;
use hashbrown::hash_table::Entry;
use luck_token::{index_to_offset, offset_to_index};
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
    /// Lua 5.5: a name a `global` statement declares.
    is_declared_global: bool,
}

/// As bytes, so a comparison checks no char boundaries.
fn entry_bytes<'names>(text: &'names str, entries: &[NameEntry], id: NameId) -> &'names [u8] {
    entries
        .get(id.index())
        .and_then(|entry| {
            text.as_bytes().get(offset_to_index(entry.start)..offset_to_index(entry.end))
        })
        .unwrap_or_default()
}

fn entry_text<'names>(text: &'names str, entries: &[NameEntry], id: NameId) -> &'names str {
    entries
        .get(id.index())
        .and_then(|entry| text.get(offset_to_index(entry.start)..offset_to_index(entry.end)))
        .unwrap_or_default()
}

impl Names {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::with_capacity(capacity.saturating_mul(8)),
            entries: Vec::with_capacity(capacity),
            table: HashTable::with_capacity(capacity),
        }
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
                let start = index_to_offset(text.len());
                text.push_str(name);
                entries.push(NameEntry {
                    start,
                    end: index_to_offset(text.len()),
                    is_assigned_global: false,
                    is_declared_global: false,
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
        self.entries.get(id.index()).is_some_and(|entry| entry.is_assigned_global)
    }

    pub(crate) fn is_declared_global(&self, id: NameId) -> bool {
        self.entries.get(id.index()).is_some_and(|entry| entry.is_declared_global)
    }

    pub(crate) fn mark_assigned_global(&mut self, id: NameId) {
        if let Some(entry) = self.entries.get_mut(id.index()) {
            entry.is_assigned_global = true;
        }
    }

    pub(crate) fn mark_declared_global(&mut self, id: NameId) {
        if let Some(entry) = self.entries.get_mut(id.index()) {
            entry.is_declared_global = true;
        }
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
        let ids: Vec<NameId> = (0..1_000u32).map(|i| names.intern(&format!("n{i}"))).collect();
        for (i, id) in ids.into_iter().enumerate() {
            assert_eq!(names.get(&format!("n{i}")), Some(id));
            assert_eq!(names.text(id), format!("n{i}"));
        }
    }
}
