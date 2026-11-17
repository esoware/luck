//! One id for each distinct name, so a pass keeps what it knows of each name
//! in a list instead of a map keyed by text.
//!
//! Interning a name makes it hold the text the table met first, so after
//! the first pass every name with that text shares one address, and finding
//! it again hashes the address instead of the text. A name that holds some
//! other copy still resolves by its text.

use rustc_hash::FxHashMap;

use crate::binding::{IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME};

#[derive(Default)]
pub(crate) struct NameTable<'ast> {
    /// Each id's text, by where it lives and how long it is.
    by_address: FxHashMap<(usize, usize), usize>,
    by_text: FxHashMap<&'ast str, usize>,
    texts: Vec<&'ast str>,
}

impl<'ast> NameTable<'ast> {
    /// A table that already holds the names the passes declare themselves,
    /// so theirs are the copies the tree's names take.
    pub(crate) fn new() -> Self {
        let mut table = Self::default();
        for text in [METHOD_RECEIVER_NAME, IMPLICIT_VARARG_TABLE_NAME, "_ENV"] {
            table.intern_text(text);
        }
        table
    }

    pub(crate) const fn len(&self) -> usize {
        self.texts.len()
    }

    /// The id of `text`, which then holds the table's copy.
    pub(crate) fn intern(&mut self, text: &mut &'ast str) -> usize {
        if let Some(&id) = self.by_address.get(&(text.as_ptr().addr(), text.len())) {
            return id;
        }
        let next_id = self.texts.len();
        let id = *self.by_text.entry(*text).or_insert(next_id);
        if let Some(&copy) = self.texts.get(id) {
            *text = copy;
        } else {
            self.texts.push(*text);
            self.by_address.insert((text.as_ptr().addr(), text.len()), id);
        }
        id
    }

    /// The id of `text`, which keeps its own copy.
    pub(crate) fn intern_text(&mut self, mut text: &'ast str) -> usize {
        self.intern(&mut text)
    }

    /// The id of `text`, if the table holds it.
    pub(crate) fn find(&self, text: &str) -> Option<usize> {
        self.by_address
            .get(&(text.as_ptr().addr(), text.len()))
            .or_else(|| self.by_text.get(text))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use super::*;

    #[test]
    fn equal_texts_share_an_id_and_a_copy() {
        let source = String::from("alpha alpha beta");
        let mut table = NameTable::new();
        let mut words = source.split(' ');
        let (mut first, mut second, mut third) =
            (words.next().unwrap(), words.next().unwrap(), words.next().unwrap());
        let alpha = table.intern(&mut first);
        assert_eq!(table.intern(&mut second), alpha);
        assert!(ptr::eq(first, second), "the second alpha takes the first one's copy");
        assert_ne!(table.intern(&mut third), alpha);
        assert_eq!(table.find("alpha"), Some(alpha));
        assert_eq!(table.find("gamma"), None);
    }

    #[test]
    fn a_name_the_passes_declare_is_held_from_the_start() {
        let source = String::from("self");
        let mut table = NameTable::new();
        let held = table.len();
        let mut name = source.as_str();
        let id = table.intern(&mut name);
        assert_eq!(table.len(), held);
        assert_eq!(table.find(METHOD_RECEIVER_NAME), Some(id));
        assert!(!ptr::eq(name, source.as_str()), "the name takes the table's copy");
    }
}
