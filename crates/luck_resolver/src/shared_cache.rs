use std::borrow::Borrow;
use std::convert::Infallible;
use std::hash::Hash;
use std::sync::Arc;

use parking_lot::RwLock;
use rustc_hash::FxHashMap;

/// How many times the caches of one resolver have been cleared, which every
/// one of them shares.
///
/// A value made from reads goes into a cache only if no clear came since the
/// reads began, so nothing a resolution read before a clear outlives it, even
/// when the resolution finishes after the clear.
#[derive(Debug, Default)]
pub(crate) struct Generation {
    clears: RwLock<u64>,
}

impl Generation {
    /// Runs `clear`, which empties every cache that shares this generation,
    /// and starts the next one, with no value going into a cache meanwhile.
    pub(crate) fn clear(&self, clear: impl FnOnce()) {
        let mut clears = self.clears.write();
        clear();
        *clears += 1;
    }
}

/// A map every thread resolving through one resolver reads and fills, which
/// keeps the first value made for each key until it's cleared.
///
/// A path keys it as an `OsString`, which hashes in one pass where a `Path`
/// hashes a component at a time, so two spellings of one path are two keys.
#[derive(Debug)]
pub(crate) struct SharedCache<K, V> {
    map: RwLock<FxHashMap<K, V>>,
    generation: Arc<Generation>,
}

impl<K, V> SharedCache<K, V> {
    pub(crate) fn new(generation: &Arc<Generation>) -> Self {
        Self { map: RwLock::new(FxHashMap::default()), generation: Arc::clone(generation) }
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

    /// The value kept for `key`, or else what `make` makes, which is kept
    /// unless another thread kept one first or a clear came while `make`
    /// ran.
    ///
    /// `make` runs outside the lock, so threads that miss `key` together each
    /// make a value.
    pub(crate) fn get_or_try_insert_with<Q, E>(
        &self,
        key: &Q,
        make: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ToOwned<Owned = K> + ?Sized,
    {
        if let Some(kept) = self.get(key) {
            return Ok(kept);
        }
        let started = *self.generation.clears.read();
        let value = make()?;
        let clears = self.generation.clears.read();
        if *clears != started {
            return Ok(value);
        }
        Ok(self.map.write().entry(key.to_owned()).or_insert(value).clone())
    }

    /// [`Self::get_or_try_insert_with`] for a `make` that can't fail.
    pub(crate) fn get_or_insert_with<Q>(&self, key: &Q, make: impl FnOnce() -> V) -> V
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ToOwned<Owned = K> + ?Sized,
    {
        let Ok(value) = self.get_or_try_insert_with(key, || Ok::<V, Infallible>(make()));
        value
    }

    /// Only inside [`Generation::clear`], so no value made before the clear
    /// goes in after it.
    pub(crate) fn clear(&self) {
        self.map.write().clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.read().len()
    }
}
