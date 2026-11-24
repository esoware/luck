use std::borrow::Borrow;
use std::hash::Hash;

use parking_lot::RwLock;
use rustc_hash::FxHashMap;

/// A map every thread resolving through one resolver reads and fills, which
/// keeps the first value made for each key until it's cleared.
///
/// A path keys it as an `OsString`, which hashes in one pass where a `Path`
/// hashes a component at a time, so two spellings of one path are two keys.
#[derive(Debug)]
pub(crate) struct SharedCache<K, V> {
    map: RwLock<FxHashMap<K, V>>,
}

impl<K, V> Default for SharedCache<K, V> {
    fn default() -> Self {
        Self { map: RwLock::new(FxHashMap::default()) }
    }
}

impl<K: Eq + Hash, V: Clone> SharedCache<K, V> {
    pub(crate) fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.map.read().get(key).cloned()
    }

    /// Returns the value kept for `key`, which is `value` unless another
    /// thread kept one first.
    pub(crate) fn insert(&self, key: K, value: V) -> V {
        self.map.write().entry(key).or_insert(value).clone()
    }

    /// `make` runs outside the lock, so threads that miss `key` together each
    /// make a value.
    pub(crate) fn get_or_insert_with<Q>(&self, key: &Q, make: impl FnOnce() -> V) -> V
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ToOwned<Owned = K> + ?Sized,
    {
        self.get(key).unwrap_or_else(|| self.insert(key.to_owned(), make()))
    }

    pub(crate) fn clear(&self) {
        self.map.write().clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.read().len()
    }
}
