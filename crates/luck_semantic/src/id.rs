use luck_token::offset_to_index;

/// Defines an index newtype over `NonZeroU32` holding `index + 1`, so
/// `Option<Id>` stays 4 bytes.
macro_rules! define_index_id {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(core::num::NonZeroU32);

        impl $name {
            /// A table never holds more rows than its chunk has bytes, so an
            /// index always fits.
            pub(crate) fn from_index(index: usize) -> Self {
                let offset = u32::try_from(index).unwrap_or(u32::MAX);
                Self(core::num::NonZeroU32::MIN.saturating_add(offset))
            }

            /// Its row in its table, for a consumer's side table.
            #[inline]
            #[must_use]
            pub fn index(self) -> usize {
                offset_to_index(self.0.get().wrapping_sub(1))
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.index())
            }
        }
    };
}

define_index_id!(
    /// A node's row in [`AstNodes`](crate::AstNodes), numbered in source
    /// preorder from the chunk's, which is 0, whether or not the table is
    /// built.
    NodeId
);
impl NodeId {
    /// The id of the node after this one, which the walk counts with one add
    /// per node.
    #[inline]
    pub(crate) const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

define_index_id!(ScopeId);
define_index_id!(SymbolId);
define_index_id!(ReferenceId);
define_index_id!(LabelId);
define_index_id!(
    /// One of a chunk's distinct names, which every symbol, reference, and
    /// label of that text shares.
    NameId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_niche_optimized() {
        assert_eq!(size_of::<Option<NodeId>>(), 4);
        assert_eq!(size_of::<Option<ScopeId>>(), 4);
        assert_eq!(size_of::<Option<SymbolId>>(), 4);
        assert_eq!(size_of::<Option<ReferenceId>>(), 4);
        assert_eq!(size_of::<Option<LabelId>>(), 4);
        assert_eq!(size_of::<Option<NameId>>(), 4);
    }

    #[test]
    fn id_roundtrips_index() {
        let id = SymbolId::from_index(7);
        assert_eq!(id.index(), 7);
        assert_eq!(format!("{id:?}"), "SymbolId(7)");
    }
}
