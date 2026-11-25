use hashbrown::HashTable;

use crate::id::NameId;

/// A chunk's distinct names, which it owns, so the tables that use them
/// outlive the tree they came from.
#[derive(Debug, Default)]
pub(crate) struct Names {
    /// Every name's text, one after another in id order.
    text: String,
    entries: Vec<NameEntry>,
    /// Whether the chunk writes each free name as a global, by id, made only
    /// once it writes one, since most chunks write none. An id past its end
    /// is never written.
    assigned_globals: Vec<bool>,
    table: HashTable<NameId>,
}

/// A name's [`Key`] and where its text starts, so finding a name of up to 16
/// bytes, as nearly every name is, compares words and reads no text.
#[derive(Debug, Clone, Copy)]
struct NameEntry {
    head: u64,
    tail: u64,
    length: u32,
    start: u32,
}

/// A name's length and its first and last eight bytes, each a little-endian
/// word with zeros past the name's end, which only the one name of up to 16
/// bytes has.
#[derive(Clone, Copy)]
struct Key {
    head: u64,
    tail: u64,
    length: u32,
}

/// At most eight bytes as a little-endian word with zeros past their end.
/// Two reads that may overlap cover every byte, so none runs past the end.
fn word(bytes: &[u8]) -> u64 {
    debug_assert!(bytes.len() <= 8, "a word holds at most eight bytes");
    if let (Some(head), Some(tail)) = (bytes.first_chunk::<4>(), bytes.last_chunk::<4>()) {
        let tail = u64::from(u32::from_le_bytes(*tail)) << (8 * (bytes.len() - 4));
        return u64::from(u32::from_le_bytes(*head)) | tail;
    }
    match *bytes {
        [] => 0,
        [first] => u64::from(first),
        [first, second] => u64::from(first) | (u64::from(second) << 8),
        [first, second, third, ..] => {
            u64::from(first) | (u64::from(second) << 8) | (u64::from(third) << 16)
        }
    }
}

/// `a * b` folded to 64 bits, the mixing step of foldhash and wyhash.
#[expect(clippy::cast_possible_truncation, reason = "the fold keeps both halves of the product")]
fn folded_multiply(a: u64, b: u64) -> u64 {
    let full = u128::from(a) * u128::from(b);
    full as u64 ^ (full >> 64) as u64
}

const SEED_HEAD: u64 = 0x243f_6a88_85a3_08d3;
const SEED_TAIL: u64 = 0x1319_8a2e_0370_7344;

impl Key {
    fn of(name: &str) -> Self {
        let bytes = name.as_bytes();
        let length =
            u32::try_from(bytes.len()).expect("a chunk's names total fewer bytes than `u32::MAX`");
        match (bytes.first_chunk::<8>(), bytes.last_chunk::<8>()) {
            (Some(head), Some(tail)) => {
                Self { head: u64::from_le_bytes(*head), tail: u64::from_le_bytes(*tail), length }
            }
            _ => Self { head: word(bytes), tail: 0, length },
        }
    }

    /// Every byte of `name`, whose key this is, goes into the hash, so names
    /// that differ only between their first and last eight bytes, as
    /// generated ones can, still spread.
    fn hash(self, name: &str) -> u64 {
        let mut tail = self.tail ^ SEED_TAIL ^ u64::from(self.length);
        if let Some(middle) = name.as_bytes().get(8..name.len().saturating_sub(8)) {
            for chunk in middle.chunks(8) {
                tail = folded_multiply(tail ^ word(chunk), SEED_TAIL);
            }
        }
        folded_multiply(self.head ^ SEED_HEAD, tail)
    }
}

impl NameEntry {
    /// Whether the entry holds `name`, whose key is `key`.
    fn holds(self, key: Key, name: &str, text: &str) -> bool {
        (self.head, self.tail, self.length) == (key.head, key.tail, key.length)
            && (key.length <= 16 || self.holds_middle(name, text))
    }

    /// Whether the bytes between a long name's first and last eight are the
    /// entry's. Few names are longer than 16 bytes, so the search keeps
    /// this compare out of its way.
    #[cold]
    fn holds_middle(self, name: &str, text: &str) -> bool {
        let (start, end) = (self.start as usize, self.length as usize - 8);
        text.as_bytes()[start + 8..start + end] == name.as_bytes()[8..end]
    }

    fn text(self, text: &str) -> &str {
        let start = self.start as usize;
        &text[start..start + self.length as usize]
    }
}

/// The hash of the name `id`, by which the table moves it as it grows.
fn rehash(text: &str, entries: &[NameEntry], id: NameId) -> u64 {
    let entry = entries[id.index()];
    let key = Key { head: entry.head, tail: entry.tail, length: entry.length };
    key.hash(entry.text(text))
}

impl Names {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::with_capacity(capacity * 8),
            entries: Vec::with_capacity(capacity),
            assigned_globals: Vec::new(),
            table: HashTable::with_capacity(capacity),
        }
    }

    /// Empties it and keeps its memory, with room for `capacity` names, as
    /// [`Self::with_capacity`] makes.
    pub(crate) fn reset(&mut self, capacity: usize) {
        let Self { text, entries, assigned_globals, table } = self;
        text.clear();
        text.reserve(capacity * 8);
        entries.clear();
        entries.reserve(capacity);
        assigned_globals.clear();
        table.clear();
        table.reserve(capacity, |&id| rehash(text, entries, id));
    }

    pub(crate) const fn len(&self) -> usize {
        self.entries.len()
    }

    /// The id of `name`, which it gets the first time it's asked for.
    pub(crate) fn intern(&mut self, name: &str) -> NameId {
        let key = Key::of(name);
        let hash = key.hash(name);
        let Self { text, entries, table, .. } = self;
        // Nearly every use of a name comes after its first, so the search
        // comes first and only a miss pays for a slot.
        if let Some(&id) = table.find(hash, |&id| entries[id.index()].holds(key, name, text)) {
            return id;
        }
        let id = NameId::from_index(entries.len());
        let start =
            u32::try_from(text.len()).expect("a chunk's names total fewer bytes than `u32::MAX`");
        text.push_str(name);
        entries.push(NameEntry { head: key.head, tail: key.tail, length: key.length, start });
        table.insert_unique(hash, id, |&id| rehash(text, entries, id));
        id
    }

    pub(crate) fn get(&self, name: &str) -> Option<NameId> {
        let key = Key::of(name);
        let Self { text, entries, table, .. } = self;
        table.find(key.hash(name), |&id| entries[id.index()].holds(key, name, text)).copied()
    }

    pub(crate) fn text(&self, id: NameId) -> &str {
        self.entries[id.index()].text(&self.text)
    }

    pub(crate) fn is_assigned_global(&self, id: NameId) -> bool {
        self.assigned_globals.get(id.index()).copied().unwrap_or_default()
    }

    pub(crate) fn mark_assigned_global(&mut self, id: NameId) {
        let index = id.index();
        if self.assigned_globals.len() <= index {
            // As many as the entries have room for, so the flags grow with
            // them rather than one write at a time.
            self.assigned_globals.resize(self.entries.capacity(), false);
        }
        self.assigned_globals[index] = true;
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

    #[test]
    fn names_of_every_length_differ_by_any_one_byte() {
        let mut texts: Vec<String> = ["\0", "\0\0", "a\0", "\0a"].map(String::from).into();
        for len in 1..=40 {
            let name: Vec<u8> = (0..len).map(|i| b'a' + i % 26).collect();
            for at in 0..usize::from(len) {
                let mut changed = name.clone();
                changed[at] = b'_';
                texts.push(String::from_utf8(changed).expect("the bytes are ASCII"));
            }
            texts.push(String::from_utf8(name).expect("the bytes are ASCII"));
        }
        let mut names = Names::default();
        let ids: Vec<NameId> = texts.iter().map(|text| names.intern(text)).collect();
        assert_eq!(names.len(), texts.len(), "each text is a name of its own");
        for (text, id) in texts.iter().zip(ids) {
            assert_eq!((names.get(text), names.text(id)), (Some(id), text.as_str()));
        }
    }

    #[test]
    fn a_reset_table_forgets_its_names() {
        let mut names = Names::default();
        let alpha = names.intern("alpha");
        names.mark_assigned_global(alpha);
        names.reset(4);
        assert_eq!((names.len(), names.get("alpha")), (0, None));
        let beta = names.intern("beta");
        assert_eq!((beta.index(), names.text(beta)), (0, "beta"));
        assert!(!names.is_assigned_global(beta), "a reset clears what the chunk writes");
    }
}
