/// A sequence of items separated by a fixed token, usually a comma.
///
/// Interior separators are implied by position; `has_trailing_separator`
/// records one after the final item (a trailing comma in a table
/// constructor, or a dangling separator kept through parse recovery). The
/// owning node implies the separator's spelling, so none is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Punctuated<T> {
    pub items: Vec<T>,
    pub has_trailing_separator: bool,
}

impl<T> Punctuated<T> {
    #[must_use]
    pub const fn empty() -> Self {
        Self { items: Vec::new(), has_trailing_separator: false }
    }

    pub fn from_item(item: T) -> Self {
        Self { items: vec![item], has_trailing_separator: false }
    }

    #[must_use]
    pub const fn from_items(items: Vec<T>) -> Self {
        Self { items, has_trailing_separator: false }
    }

    pub fn push(&mut self, item: T) {
        self.items.push(item);
    }

    /// Replaces each item with `rewrite(item)`, keeping the trailing
    /// separator.
    #[must_use]
    pub fn map_items<Rewrite>(mut self, rewrite: Rewrite) -> Self
    where
        Rewrite: FnMut(T) -> T,
    {
        self.items = self.items.into_iter().map(rewrite).collect();
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&T> {
        self.items.get(index)
    }

    #[must_use]
    pub fn first(&self) -> Option<&T> {
        self.items.first()
    }

    #[must_use]
    pub fn last(&self) -> Option<&T> {
        self.items.last()
    }

    #[must_use]
    pub fn into_items(self) -> Vec<T> {
        self.items
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
