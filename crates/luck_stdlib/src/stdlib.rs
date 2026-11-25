use std::fmt;
use std::sync::Arc;

use luck_allocator::Allocator;
use luck_diagnostics::Diagnostic;
use luck_token::Variant;
use self_cell::{MutBorrow, self_cell};

use crate::layering::layer;
use crate::stdlib_catalog::library_for;
use crate::stdlib_definition::StdlibDefinition;
use crate::stdlib_model::StdlibLibrary;

/// A variant's standard library, with the std definitions a project names
/// layered over it, which semantic analysis reads as it reads a built-in
/// one. A clone shares it.
#[derive(Clone)]
pub struct Stdlib(Layers);

#[derive(Clone)]
enum Layers {
    BuiltIn(&'static StdlibLibrary<'static>),
    Layered(Arc<LayeredLibrary>),
}

/// The arena sits in a [`MutBorrow`], which lends it once, to the layering,
/// so nothing can reach it again from another thread, and the cell is `Sync`
/// with no `unsafe`.
struct LayeredOwner {
    arena: MutBorrow<Allocator>,
    /// The library the layered one shares every entry it leaves alone with.
    base: Stdlib,
}

self_cell!(
    struct LayeredLibrary {
        owner: LayeredOwner,
        #[covariant]
        dependent: StdlibLibrary,
    }
);

impl Stdlib {
    /// `variant`'s built-in library.
    pub const fn new(variant: Variant) -> Self {
        Self(Layers::BuiltIn(library_for(variant)))
    }

    pub fn library(&self) -> &StdlibLibrary<'_> {
        match &self.0 {
            Layers::BuiltIn(library) => library,
            Layers::Layered(layered) => layered.borrow_dependent(),
        }
    }

    pub fn variant(&self) -> Variant {
        self.library().variant
    }

    /// This library with `definition` layered over it.
    ///
    /// What the definition declares replaces what this library declares
    /// under the same name, except that a namespace over a namespace, or a
    /// shape over a shape, keeps the members it doesn't declare, and the
    /// `shape`, `extends`, or deprecation it doesn't set.
    ///
    /// A shape or a constant set the definition names comes from it or from
    /// this library. One that only Roblox's library declares is left
    /// unresolved, so one definition can serve both variants: the shape has
    /// no members, and a param that takes the set takes any string.
    ///
    /// # Errors
    ///
    /// Returns E014 when the definition names a shape or a constant set that
    /// no library declares, or gives a table more than 65535 entries.
    pub fn layer(&self, definition: &StdlibDefinition) -> Result<Self, Diagnostic> {
        let owner = LayeredOwner { arena: MutBorrow::new(Allocator::new()), base: self.clone() };
        let layered = LayeredLibrary::try_new(owner, |built_from| {
            layer(built_from.arena.borrow_mut(), built_from.base.library(), definition)
        })?;
        Ok(Self(Layers::Layered(Arc::new(layered))))
    }
}

impl fmt::Debug for Stdlib {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut layer_count = 0usize;
        let mut current = self;
        while let Layers::Layered(layered) = &current.0 {
            layer_count += 1;
            current = &layered.borrow_owner().base;
        }
        f.debug_struct("Stdlib")
            .field("variant", &self.variant())
            .field("layer_count", &layer_count)
            .finish()
    }
}
