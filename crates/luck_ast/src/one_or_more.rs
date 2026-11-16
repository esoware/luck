use core::ops::{Deref, DerefMut};
use core::{fmt, mem, slice};

/// A list that is never empty. A lone item sits inline, so it costs no
/// allocation of its own. Derefs to a slice of every item.
pub struct OneOrMore<T>(Repr<T>);

enum Repr<T> {
    One(T),
    /// Never empty.
    Many(Vec<T>),
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
        Self(Repr::Many(items))
    }

    /// `None` when `items` is empty.
    #[must_use]
    pub fn from_vec(mut items: Vec<T>) -> Option<Self> {
        if items.len() > 1 {
            return Some(Self(Repr::Many(items)));
        }
        items.pop().map(Self::new)
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        match self.0 {
            Repr::One(item) => vec![item],
            Repr::Many(items) => items,
        }
    }

    pub fn push(&mut self, item: T) {
        self.0 = match mem::replace(&mut self.0, Repr::Many(Vec::new())) {
            Repr::One(first) => Repr::Many(vec![first, item]),
            Repr::Many(mut items) => {
                items.push(item);
                Repr::Many(items)
            }
        };
    }

    /// Removes every item after the first and returns them in order.
    pub fn split_off_rest(&mut self) -> Vec<T> {
        match &mut self.0 {
            Repr::One(_) => Vec::new(),
            Repr::Many(items) => items.split_off(1),
        }
    }

    #[must_use]
    pub fn first(&self) -> &T {
        match &self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` is never empty")]
            Repr::Many(items) => &items[0],
        }
    }

    #[must_use]
    pub fn last(&self) -> &T {
        match &self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` is never empty")]
            Repr::Many(items) => &items[items.len().saturating_sub(1)],
        }
    }

    #[must_use]
    pub fn last_mut(&mut self) -> &mut T {
        match &mut self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` is never empty")]
            Repr::Many(items) => {
                let last = items.len().saturating_sub(1);
                &mut items[last]
            }
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

impl<T: Clone> Clone for OneOrMore<T> {
    fn clone(&self) -> Self {
        Self(match &self.0 {
            Repr::One(item) => Repr::One(item.clone()),
            Repr::Many(items) => Repr::Many(items.clone()),
        })
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
    fn keeps_one_item_inline_and_grows_past_it() {
        let mut list = OneOrMore::new(1u8);
        assert_eq!(*list, [1]);
        list.push(2);
        list.push(3);
        assert_eq!(*list, [1, 2, 3]);
        assert_eq!((*list.first(), *list.last()), (1, 3));
        *list.last_mut() = 4;
        assert_eq!(list.into_vec(), [1, 2, 4]);
    }

    #[test]
    fn builds_from_a_vec_of_any_nonzero_length() {
        assert!(OneOrMore::<u8>::from_vec(Vec::new()).is_none());
        assert_eq!(OneOrMore::from_vec(vec![7u8]), Some(OneOrMore::new(7)));
        let list = OneOrMore::from_vec(vec![1u8, 2]).expect("two items");
        assert_eq!(*list, [1, 2]);
    }

    #[test]
    fn builds_from_a_first_item_in_one_exact_allocation() {
        assert_eq!(OneOrMore::from_first(1u8, []), OneOrMore::new(1));
        let list = OneOrMore::from_first(1u8, [2, 3]);
        assert_eq!(*list, [1, 2, 3]);
        assert_eq!(list.into_vec().capacity(), 3);
    }

    #[test]
    fn splitting_off_the_rest_keeps_the_first() {
        let mut single = OneOrMore::new(1u8);
        assert_eq!(single.split_off_rest(), Vec::<u8>::new());
        assert_eq!(*single, [1]);
        let mut list = OneOrMore::from_first(1u8, [2, 3]);
        assert_eq!(list.split_off_rest(), [2, 3]);
        assert_eq!(*list, [1]);
        assert_eq!((*list.first(), *list.last()), (1, 1));
        list.push(4);
        assert_eq!(list, OneOrMore::from_first(1, [4]));
    }
}
