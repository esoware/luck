use core::fmt;
use core::ops::{Deref, DerefMut};
use core::slice;

use luck_allocator::{Allocator, List, ListIntoIter};

/// A [`List`] that is never empty: every way to build or change one keeps an
/// item in it. Derefs to a slice of every item.
#[derive(PartialEq, Eq)]
pub struct OneOrMore<'ast, T>(List<'ast, T>);

impl<'ast, T> OneOrMore<'ast, T> {
    #[must_use]
    pub fn new(item: T, allocator: &'ast Allocator) -> Self {
        Self(List::from_array_in([item], allocator))
    }

    /// `None` when `items` is empty.
    #[must_use]
    pub fn from_list(items: List<'ast, T>) -> Option<Self> {
        (!items.is_empty()).then_some(Self(items))
    }

    /// See [`List::push`].
    pub fn push(&mut self, item: T, allocator: &'ast Allocator) {
        self.0.push(item, allocator);
    }

    /// See [`List::insert`].
    pub fn insert(&mut self, index: usize, item: T, allocator: &'ast Allocator) {
        self.0.insert(index, item, allocator);
    }

    /// Removes every item after the first and returns them in order, without
    /// allocating, as [`List::split_off`] does.
    #[must_use]
    pub fn split_off_rest(&mut self) -> List<'ast, T> {
        self.0.split_off(1)
    }

    #[must_use]
    pub fn first(&self) -> &T {
        #[expect(clippy::indexing_slicing, reason = "the list is never empty")]
        &self.0[0]
    }

    #[must_use]
    pub fn last(&self) -> &T {
        #[expect(clippy::indexing_slicing, reason = "the list is never empty")]
        &self.0[self.0.len().saturating_sub(1)]
    }

    #[must_use]
    pub fn last_mut(&mut self) -> &mut T {
        let last = self.0.len().saturating_sub(1);
        #[expect(clippy::indexing_slicing, reason = "the list is never empty")]
        &mut self.0[last]
    }
}

impl<T> Deref for OneOrMore<'_, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> DerefMut for OneOrMore<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<'ast, T> IntoIterator for OneOrMore<'ast, T> {
    type Item = T;
    type IntoIter = ListIntoIter<'ast, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'list, T> IntoIterator for &'list OneOrMore<'_, T> {
    type Item = &'list T;
    type IntoIter = slice::Iter<'list, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'list, T> IntoIterator for &'list mut OneOrMore<'_, T> {
    type Item = &'list mut T;
    type IntoIter = slice::IterMut<'list, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T: fmt::Debug> fmt::Debug for OneOrMore<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_at_either_end_and_keeps_its_first_and_last() {
        let allocator = Allocator::new();
        let mut list = OneOrMore::new(2u8, &allocator);
        assert_eq!((*list.first(), *list.last()), (2, 2));
        list.push(3, &allocator);
        list.insert(0, 1, &allocator);
        assert_eq!(*list, [1, 2, 3]);
        assert_eq!((*list.first(), *list.last()), (1, 3));
        *list.last_mut() = 4;
        assert_eq!(list.into_iter().collect::<Vec<_>>(), [1, 2, 4]);
    }

    #[test]
    fn builds_from_any_list_but_an_empty_one() {
        let allocator = Allocator::new();
        assert!(OneOrMore::<u8>::from_list(List::new()).is_none());
        let list = OneOrMore::from_list(List::from_array_in([7u8, 8], &allocator));
        assert_eq!(list.as_deref(), Some([7, 8].as_slice()));
    }
}
