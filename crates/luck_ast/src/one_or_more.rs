use core::ops::{Deref, DerefMut};
use core::{fmt, mem, slice};

/// A list that is never empty. A lone item sits inline, so it costs no
/// allocation of its own, and two or more share one slice of exactly their
/// length.
///
/// Derefs to a slice of every item. [`OneOrMore::push`] copies that slice,
/// so build a long list with [`OneOrMore::from_vec`].
#[derive(Clone)]
pub struct OneOrMore<T>(Repr<T>);

#[derive(Clone)]
enum Repr<T> {
    One(T),
    /// At least two items.
    Many(Box<[T]>),
}

impl<T> OneOrMore<T> {
    #[must_use]
    pub const fn new(item: T) -> Self {
        Self(Repr::One(item))
    }

    /// `first` and then `rest`, in one allocation when `rest` knows its
    /// length.
    pub fn from_first<Rest>(first: T, rest: Rest) -> Self
    where
        Rest: IntoIterator<Item = T>,
    {
        let mut others = rest.into_iter();
        let Some(second) = others.next() else {
            return Self::new(first);
        };
        let mut items = Vec::with_capacity(others.size_hint().0.saturating_add(2));
        items.push(first);
        items.push(second);
        items.extend(others);
        Self(Repr::Many(items.into_boxed_slice()))
    }

    /// `None` when `items` is empty. Spare capacity is given back.
    #[must_use]
    pub fn from_vec(mut items: Vec<T>) -> Option<Self> {
        if items.len() > 1 {
            return Some(Self(Repr::Many(items.into_boxed_slice())));
        }
        items.pop().map(Self::new)
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        match self.0 {
            Repr::One(item) => vec![item],
            Repr::Many(items) => items.into_vec(),
        }
    }

    pub fn push(&mut self, item: T) {
        // An empty boxed slice owns no allocation, so it holds the place for
        // free.
        let mut grown = match mem::replace(&mut self.0, Repr::Many(Box::default())) {
            Repr::One(first) => {
                let mut pair = Vec::with_capacity(2);
                pair.push(first);
                pair
            }
            Repr::Many(items) => {
                let mut longer = items.into_vec();
                longer.reserve_exact(1);
                longer
            }
        };
        grown.push(item);
        self.0 = Repr::Many(grown.into_boxed_slice());
    }

    /// Puts `front`'s items before these, in one allocation.
    pub fn prepend(&mut self, front: Self) {
        let back = mem::replace(&mut self.0, Repr::Many(Box::default()));
        let mut items = front.into_vec();
        match back {
            Repr::One(item) => {
                items.reserve_exact(1);
                items.push(item);
            }
            Repr::Many(rest) => {
                items.reserve_exact(rest.len());
                items.extend(rest.into_vec());
            }
        }
        self.0 = Repr::Many(items.into_boxed_slice());
    }

    #[must_use]
    pub fn first(&self) -> &T {
        match &self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` holds at least two items")]
            Repr::Many(items) => &items[0],
        }
    }

    #[must_use]
    pub fn last(&self) -> &T {
        match &self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` holds at least two items")]
            Repr::Many(items) => &items[items.len().saturating_sub(1)],
        }
    }

    #[must_use]
    pub fn last_mut(&mut self) -> &mut T {
        match &mut self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` holds at least two items")]
            Repr::Many(items) => &mut items[items.len().saturating_sub(1)],
        }
    }
}

impl<T> Deref for OneOrMore<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        match &self.0 {
            Repr::One(item) => slice::from_ref(item),
            Repr::Many(items) => items,
        }
    }
}

impl<T> DerefMut for OneOrMore<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match &mut self.0 {
            Repr::One(item) => slice::from_mut(item),
            Repr::Many(items) => items,
        }
    }
}

impl<'list, T> IntoIterator for &'list OneOrMore<T> {
    type Item = &'list T;
    type IntoIter = slice::Iter<'list, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'list, T> IntoIterator for &'list mut OneOrMore<T> {
    type Item = &'list mut T;
    type IntoIter = slice::IterMut<'list, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T: PartialEq> PartialEq for OneOrMore<T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: Eq> Eq for OneOrMore<T> {}

impl<T: fmt::Debug> fmt::Debug for OneOrMore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_one_item_inline_and_more_in_an_exact_slice() {
        let mut list = OneOrMore::new(1u8);
        assert_eq!(*list, [1]);
        list.push(2);
        list.push(3);
        assert_eq!(*list, [1, 2, 3]);
        assert_eq!((*list.first(), *list.last()), (1, 3));
        assert_eq!(list.into_vec().capacity(), 3);
    }

    #[test]
    fn builds_from_a_vec_of_any_nonzero_length() {
        assert!(OneOrMore::<u8>::from_vec(Vec::new()).is_none());
        assert_eq!(OneOrMore::from_vec(vec![7u8]), Some(OneOrMore::new(7)));
        let mut spare = Vec::with_capacity(16);
        spare.extend([1u8, 2]);
        let list = OneOrMore::from_vec(spare).expect("two items");
        assert_eq!(list.into_vec().capacity(), 2);
    }

    #[test]
    fn prepends_a_list_of_any_length() {
        let mut single = OneOrMore::new(2u8);
        single.prepend(OneOrMore::new(1));
        assert_eq!(*single, [1, 2]);
        let mut pair = OneOrMore::from_first(3u8, [4]);
        pair.prepend(OneOrMore::from_first(1, [2]));
        assert_eq!(*pair, [1, 2, 3, 4]);
        assert_eq!(pair.into_vec().capacity(), 4);
    }
}
