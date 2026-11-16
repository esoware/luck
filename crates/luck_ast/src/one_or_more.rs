use core::ops::{Deref, DerefMut};
use core::{fmt, iter, mem, slice};

/// A list that is never empty. A lone item sits inline, so it costs no
/// allocation of its own, and two or more share one slice of exactly their
/// length.
///
/// Derefs to a slice of every item. A push onto two or more leaves room for
/// the pushes after it, so a list built one item at a time takes time linear
/// in its length. [`OneOrMore::from_vec`] gives the room back.
pub struct OneOrMore<T>(Repr<T>);

enum Repr<T> {
    One(T),
    Many(Many<T>),
}

/// At least two items. Its two shapes sit a level under [`Repr`], so telling
/// one item from more costs what it would with one shape.
enum Many<T> {
    Exact(Box<[T]>),
    /// The items and the room a push left: the spare capacity of `slots`
    /// after them, and `start` fillers before them.
    Grown {
        slots: Vec<T>,
        start: usize,
    },
}

impl<T> Many<T> {
    fn as_slice(&self) -> &[T] {
        match self {
            Self::Exact(items) => items,
            Self::Grown { slots, start } => slots.get(*start..).unwrap_or_default(),
        }
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        match self {
            Self::Exact(items) => items,
            Self::Grown { slots, start } => slots.get_mut(*start..).unwrap_or_default(),
        }
    }

    fn into_vec(self) -> Vec<T> {
        match self {
            Self::Exact(items) => items.into_vec(),
            Self::Grown { mut slots, start } => {
                drop(slots.drain(..start));
                slots
            }
        }
    }
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
        Self(Repr::Many(Many::Exact(items.into_boxed_slice())))
    }

    /// `None` when `items` is empty. Spare capacity is given back.
    #[must_use]
    pub fn from_vec(mut items: Vec<T>) -> Option<Self> {
        if items.len() > 1 {
            return Some(Self(Repr::Many(Many::Exact(items.into_boxed_slice()))));
        }
        items.pop().map(Self::new)
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        match self.0 {
            Repr::One(item) => vec![item],
            Repr::Many(many) => many.into_vec(),
        }
    }

    /// An empty boxed slice owns no allocation, so it holds the place of a
    /// list being rebuilt for free.
    fn take_repr(&mut self) -> Repr<T> {
        mem::replace(&mut self.0, Repr::Many(Many::Exact(Box::default())))
    }

    pub fn push(&mut self, item: T) {
        if let Repr::Many(Many::Grown { slots, .. }) = &mut self.0 {
            slots.push(item);
            return;
        }
        self.0 = Repr::Many(match self.take_repr() {
            Repr::One(first) => Many::Exact(Box::new([first, item])),
            Repr::Many(many) => {
                let mut slots = many.into_vec();
                slots.push(item);
                Many::Grown { slots, start: 0 }
            }
        });
    }

    /// Puts `item` before the others. `filler` makes the values that hold
    /// the room left before it, which nothing reads.
    pub(crate) fn push_front<Filler>(&mut self, item: T, filler: Filler)
    where
        Filler: FnMut() -> T,
    {
        if let Repr::Many(Many::Grown { slots, start }) = &mut self.0
            && let Some(before) = start.checked_sub(1)
            && let Some(slot) = slots.get_mut(before)
        {
            *slot = item;
            *start = before;
            return;
        }
        self.0 = Repr::Many(match self.take_repr() {
            Repr::One(second) => Many::Exact(Box::new([item, second])),
            Repr::Many(many) => {
                let items = many.into_vec();
                // As many fillers as items, so the room doubles each time it
                // runs out.
                let start = items.len();
                let mut slots = Vec::with_capacity(start.saturating_mul(2).saturating_add(1));
                slots.extend(iter::repeat_with(filler).take(start));
                slots.push(item);
                slots.extend(items);
                Many::Grown { slots, start }
            }
        });
    }

    /// Puts `front`'s items before these, in one allocation.
    pub fn prepend(&mut self, front: Self) {
        let mut items = front.into_vec();
        match self.take_repr() {
            Repr::One(item) => {
                items.reserve_exact(1);
                items.push(item);
            }
            Repr::Many(many) => {
                let rest = many.into_vec();
                items.reserve_exact(rest.len());
                items.extend(rest);
            }
        }
        self.0 = Repr::Many(Many::Exact(items.into_boxed_slice()));
    }

    #[must_use]
    pub fn first(&self) -> &T {
        match &self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` holds at least two items")]
            Repr::Many(many) => &many.as_slice()[0],
        }
    }

    #[must_use]
    pub fn last(&self) -> &T {
        match &self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` holds at least two items")]
            Repr::Many(many) => {
                let items = many.as_slice();
                &items[items.len().saturating_sub(1)]
            }
        }
    }

    #[must_use]
    pub fn last_mut(&mut self) -> &mut T {
        match &mut self.0 {
            Repr::One(item) => item,
            #[expect(clippy::indexing_slicing, reason = "a `Many` holds at least two items")]
            Repr::Many(many) => {
                let items = many.as_mut_slice();
                &mut items[items.len().saturating_sub(1)]
            }
        }
    }
}

impl<T> Deref for OneOrMore<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        match &self.0 {
            Repr::One(item) => slice::from_ref(item),
            Repr::Many(many) => many.as_slice(),
        }
    }
}

impl<T> DerefMut for OneOrMore<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match &mut self.0 {
            Repr::One(item) => slice::from_mut(item),
            Repr::Many(many) => many.as_mut_slice(),
        }
    }
}

/// A clone holds no room.
impl<T: Clone> Clone for OneOrMore<T> {
    fn clone(&self) -> Self {
        Self(match &self.0 {
            Repr::One(item) => Repr::One(item.clone()),
            Repr::Many(many) => Repr::Many(Many::Exact(Box::from(many.as_slice()))),
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
    fn keeps_one_item_inline_and_a_pair_in_an_exact_slice() {
        let mut list = OneOrMore::new(1u8);
        assert_eq!(*list, [1]);
        list.push(2);
        assert_eq!(*list, [1, 2]);
        assert_eq!((*list.first(), *list.last()), (1, 2));
        assert_eq!(list.into_vec().capacity(), 2);
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
        let mut grown = OneOrMore::from_first(4u8, [5]);
        grown.push_front(3, || 0);
        grown.prepend(OneOrMore::from_first(1, [2]));
        assert_eq!(*grown, [1, 2, 3, 4, 5]);
        assert_eq!(grown.into_vec().capacity(), 5);
    }

    /// How often pushing `items` moves the list to a new allocation, which a
    /// slice that starts and ends somewhere else shows.
    fn moves_while_pushing<Items>(
        list: &mut OneOrMore<u32>,
        items: Items,
        push: fn(&mut OneOrMore<u32>, u32),
    ) -> usize
    where
        Items: IntoIterator<Item = u32>,
    {
        let mut moves = 0usize;
        for item in items {
            let (start, end) = (list.as_ptr(), list.as_ptr_range().end);
            push(list, item);
            if list.as_ptr() != start && list.as_ptr_range().end != end {
                moves = moves.saturating_add(1);
            }
        }
        moves
    }

    #[test]
    fn pushes_at_either_end_move_the_items_a_logarithmic_number_of_times() {
        let mut pushed = OneOrMore::new(0u32);
        let push_moves = moves_while_pushing(&mut pushed, 1..=100_000, OneOrMore::push);
        assert!(push_moves <= 20, "{push_moves} moves");
        assert!(pushed.iter().copied().eq(0..=100_000));
        assert_eq!((*pushed.first(), *pushed.last()), (0, 100_000));

        let mut fronted = OneOrMore::new(100_000u32);
        let push_front_moves =
            moves_while_pushing(&mut fronted, (0..100_000).rev(), |list, item| {
                list.push_front(item, || u32::MAX);
            });
        assert!(push_front_moves <= 20, "{push_front_moves} moves");
        assert!(fronted.iter().copied().eq(0..=100_000));
        assert_eq!((*fronted.first(), *fronted.last()), (0, 100_000));
        *fronted.last_mut() = 7;
        assert_eq!(fronted.into_vec().last(), Some(&7));
    }

    #[test]
    fn pushes_at_both_ends_keep_every_item_in_order() {
        let mut list = OneOrMore::new(0i32);
        for (back, front) in (1i32..=500i32).zip((-500i32..0i32).rev()) {
            list.push(back);
            list.push_front(front, || i32::MAX);
        }
        assert!(list.iter().copied().eq(-500i32..=500i32));
        let copy = list.clone();
        assert_eq!(copy, list);
        assert_eq!(copy.into_vec().capacity(), 1001);
        assert!(list.into_vec().into_iter().eq(-500i32..=500i32));
    }
}
