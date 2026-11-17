//! One id for each distinct name text, so a pass keeps what it knows of each
//! name in a list instead of a map keyed by text.

use rustc_hash::FxHashMap;

#[derive(Default)]
pub(crate) struct NameTable<'ast> {
    ids: FxHashMap<&'ast str, usize>,
}

impl<'ast> NameTable<'ast> {
    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    pub(crate) fn clear(&mut self) {
        self.ids.clear();
    }

    pub(crate) fn intern(&mut self, text: &'ast str) -> usize {
        let next_id = self.ids.len();
        *self.ids.entry(text).or_insert(next_id)
    }

    /// The id of `text`, if the table holds it.
    pub(crate) fn find(&self, text: &str) -> Option<usize> {
        self.ids.get(text).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_texts_share_an_id() {
        let source = String::from("alpha alpha beta");
        let mut table = NameTable::default();
        let mut words = source.split(' ');
        let (first, second, third) =
            (words.next().unwrap(), words.next().unwrap(), words.next().unwrap());
        let alpha = table.intern(first);
        assert_eq!(table.intern(second), alpha);
        assert_ne!(table.intern(third), alpha);
        assert_eq!(table.find("alpha"), Some(alpha));
        assert_eq!(table.find("gamma"), None);
    }
}
