use std::fmt;
use std::sync::{Arc, OnceLock};

use luck_allocator::Allocator;
use luck_diagnostics::Diagnostic;
use self_cell::{MutBorrow, self_cell};

use crate::definitions::{luau_definitions, roblox_definitions};
use crate::security_level::SecurityLevel;
use crate::stdlib_model::StdlibLibrary;
use crate::translate::layer;

/// A standard library built from definition files: Luau's, with the ones a
/// project loads layered over it, which semantic analysis reads. A clone
/// shares it.
#[derive(Clone)]
pub struct Stdlib(Arc<LayeredLibrary>);

/// The arena sits in a [`MutBorrow`], which lends it once, to the layering,
/// so nothing can reach it again from another thread, and the cell is `Sync`
/// with no `unsafe`.
struct LayeredOwner {
    arena: MutBorrow<Allocator>,
    /// The library the layered one shares every entry it leaves alone with.
    base: Option<Stdlib>,
}

self_cell!(
    struct LayeredLibrary {
        owner: LayeredOwner,
        #[covariant]
        dependent: StdlibLibrary,
    }
);

static LUAU_LIBRARY: OnceLock<Stdlib> = OnceLock::new();

/// Roblox's API over Luau's library at each level of [`SecurityLevel::ALL`].
static ROBLOX_LIBRARIES: [OnceLock<Stdlib>; SecurityLevel::ALL.len()] =
    [const { OnceLock::new() }; SecurityLevel::ALL.len()];

/// Luau's own library, `luau.d.luau`'s, which every project's starts from,
/// built the first time a process asks for it.
///
/// # Panics
///
/// Never, since the definitions built into the crate parse, as its tests
/// check.
pub fn luau_library() -> &'static StdlibLibrary<'static> {
    built_in_luau().library()
}

/// Luau's library with Roblox's API for a script at `security` layered over
/// it, the definitions [`roblox_definitions`] gives, built the first time a
/// process asks for it.
///
/// # Panics
///
/// Never, since the definitions built into the crate parse, as its tests
/// check.
pub fn roblox_library(security: SecurityLevel) -> &'static StdlibLibrary<'static> {
    built_in_roblox(security).library()
}

/// The library a Roblox script runs against when `has_roblox`, which is
/// [`roblox_library`] at [`SecurityLevel::None`], and else
/// [`luau_library`].
pub fn library_for(has_roblox: bool) -> &'static StdlibLibrary<'static> {
    if has_roblox { roblox_library(SecurityLevel::None) } else { luau_library() }
}

fn built_in_luau() -> &'static Stdlib {
    LUAU_LIBRARY.get_or_init(|| {
        Stdlib::layered(None, false, luau_definitions()).expect("luau.d.luau parses")
    })
}

fn built_in_roblox(security: SecurityLevel) -> &'static Stdlib {
    ROBLOX_LIBRARIES[security as usize].get_or_init(|| built_in_luau().roblox_over(security))
}

impl Stdlib {
    /// Luau's own library, as [`luau_library`] gives it.
    pub fn luau() -> Self {
        built_in_luau().clone()
    }

    pub fn library(&self) -> &StdlibLibrary<'_> {
        self.0.borrow_dependent()
    }

    /// This library with Roblox's API for a script at `security` layered
    /// over it, which over Luau's alone a process builds once at each level.
    ///
    /// # Panics
    ///
    /// Never, since the definitions built into the crate parse, as its tests
    /// check.
    #[must_use]
    pub fn with_roblox(&self, security: SecurityLevel) -> Self {
        if Arc::ptr_eq(&self.0, &built_in_luau().0) {
            return built_in_roblox(security).clone();
        }
        self.roblox_over(security)
    }

    fn roblox_over(&self, security: SecurityLevel) -> Self {
        Self::layered(Some(self.clone()), true, roblox_definitions(security))
            .expect("roblox.d.luau parses at every level")
    }

    /// This library with the definition file `source` layered over it.
    ///
    /// What the file declares replaces what this library declares under the
    /// same name, as Luau's `loadDefinitionFile` treats a redeclared global,
    /// and `typeof` of a global reads the one declared before it, here or
    /// below.
    ///
    /// # Errors
    ///
    /// Returns the E008 of each place `source` doesn't parse.
    pub fn layer(&self, source: &str) -> Result<Self, Vec<Diagnostic>> {
        Self::layered(Some(self.clone()), self.library().has_roblox, source)
    }

    fn layered(
        base: Option<Self>,
        has_roblox: bool,
        source: &str,
    ) -> Result<Self, Vec<Diagnostic>> {
        let owner = LayeredOwner { arena: MutBorrow::new(Allocator::new()), base };
        let layered = LayeredLibrary::try_new(owner, |built_from| {
            let below = built_from.base.as_ref().map(Self::library);
            layer(built_from.arena.borrow_mut(), below, has_roblox, source)
        })?;
        Ok(Self(Arc::new(layered)))
    }
}

impl fmt::Debug for Stdlib {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut layer_count = 0usize;
        let mut current = Some(self);
        while let Some(layered) = current {
            layer_count += 1;
            current = layered.0.borrow_owner().base.as_ref();
        }
        f.debug_struct("Stdlib")
            .field("has_roblox", &self.library().has_roblox)
            .field("layer_count", &layer_count)
            .finish()
    }
}
