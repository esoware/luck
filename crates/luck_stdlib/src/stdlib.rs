#![expect(
    clippy::mem_forget,
    reason = "`self_cell!`'s builders forget their drop guard once the cell is built"
)]

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::fmt;

use luck_allocator::Allocator;
use luck_diagnostics::Diagnostic;
use luck_token::LuaTarget;
use self_cell::{MutBorrow, self_cell};

use crate::stdlib_catalog::library_for;
use crate::stdlib_definition::{
    ArgKind, Constant, Constants, Deprecation, Entry, Function, Reference, StdlibDefinition, Value,
    place,
};
use crate::stdlib_model::{
    MAX_MAP_ENTRIES, StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction,
    StdlibLibrary, StdlibMap, StdlibNamespace, StdlibParam, StdlibReplacement, StdlibShape,
    StdlibSignature, StdlibValue,
};

/// A target's standard library, with the std definitions a project names
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
    /// `target`'s built-in library.
    #[must_use]
    pub const fn new(target: LuaTarget) -> Self {
        Self(Layers::BuiltIn(library_for(target)))
    }

    #[must_use]
    pub fn library(&self) -> &StdlibLibrary<'_> {
        match &self.0 {
            Layers::BuiltIn(library) => library,
            Layers::Layered(layered) => layered.borrow_dependent(),
        }
    }

    #[must_use]
    pub fn target(&self) -> LuaTarget {
        self.library().target
    }

    /// This library with `definition` layered over it.
    ///
    /// What the definition declares replaces what this library declares
    /// under the same name, except that a namespace over a namespace, or a
    /// shape over a shape, keeps the members it doesn't declare, and the
    /// `shape`, `extends`, or deprecation it doesn't set.
    ///
    /// A shape or a constant set the definition names comes from it or from
    /// this library. One that only another target's library declares is
    /// left unresolved, so one definition can serve every target: the shape
    /// has no members, and a param that takes the set takes any string.
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
            layer_count = layer_count.saturating_add(1);
            current = &layered.borrow_owner().base;
        }
        f.debug_struct("Stdlib")
            .field("target", &self.target())
            .field("layer_count", &layer_count)
            .finish()
    }
}

/// `definition` layered over `base`, as [`Stdlib::layer`] describes, in
/// `allocator`.
///
/// # Errors
///
/// As for [`Stdlib::layer`].
pub(crate) fn layer<'lib>(
    allocator: &'lib Allocator,
    base: &StdlibLibrary<'lib>,
    definition: &StdlibDefinition,
) -> Result<StdlibLibrary<'lib>, Diagnostic> {
    let mut layering = Layering { allocator, base, definition, constant_sets: base.constant_sets };

    let mut constant_sets: BTreeMap<&'lib str, &'lib [StdlibConstant<'lib>]> =
        base.constant_sets.iter().map(|(name, values)| (name, *values)).collect();
    for (name, values) in definition.constant_sets() {
        let built = layering.constants(values);
        layering.insert(&mut constant_sets, name, built);
    }
    layering.constant_sets = layering.map(constant_sets)?;

    let mut shapes: BTreeMap<&'lib str, StdlibShape<'lib>> =
        base.shapes.iter().map(|(name, shape)| (name, *shape)).collect();
    for (name, shape) in definition.shapes() {
        let below = shapes
            .get(name.as_str())
            .copied()
            .unwrap_or(StdlibShape { members: StdlibMap::EMPTY, extends: None });
        let built = StdlibShape {
            members: layering.members(below.members, &shape.members, &format!("{name}:"))?,
            extends: layering.shape_name(shape.extends.as_ref(), name)?.or(below.extends),
        };
        layering.insert(&mut shapes, name, built);
    }

    Ok(StdlibLibrary {
        target: base.target,
        globals: layering.members(base.globals, definition.globals(), "")?,
        shapes: layering.map(shapes)?,
        constant_sets: layering.constant_sets,
    })
}

struct Layering<'lib, 'layering> {
    allocator: &'lib Allocator,
    base: &'layering StdlibLibrary<'lib>,
    definition: &'layering StdlibDefinition,
    /// The base's sets with the definition's over them, once they're
    /// layered, which every param resolves its set from.
    constant_sets: StdlibMap<'lib, &'lib [StdlibConstant<'lib>]>,
}

impl<'lib> Layering<'lib, '_> {
    /// `below`'s members with `layer`'s over them, where `prefix` begins the
    /// path of each.
    ///
    /// # Errors
    ///
    /// Fails on a member that names a shape or a constant set no library
    /// declares, or on more members than a map holds.
    fn members(
        &self,
        below: StdlibMap<'lib, StdlibEntry<'lib>>,
        layer: &BTreeMap<String, Entry>,
        prefix: &str,
    ) -> Result<StdlibMap<'lib, StdlibEntry<'lib>>, Diagnostic> {
        let mut members: BTreeMap<&'lib str, StdlibEntry<'lib>> =
            below.iter().map(|(name, entry)| (name, *entry)).collect();
        for (name, entry) in layer {
            let path = format!("{prefix}{name}");
            let built = match entry {
                Entry::Function(function) => {
                    StdlibEntry::Function(self.allocator.alloc(self.function(function, &path)?))
                }
                Entry::Constant(value) => {
                    StdlibEntry::Constant(self.allocator.alloc(self.value(value, &path)?))
                }
                Entry::Property(value) => {
                    StdlibEntry::Property(self.allocator.alloc(self.value(value, &path)?))
                }
                Entry::Namespace(namespace) => {
                    let under = match members.get(name.as_str()) {
                        Some(StdlibEntry::Namespace(found)) => **found,
                        Some(
                            StdlibEntry::Function(_)
                            | StdlibEntry::Constant(_)
                            | StdlibEntry::Property(_),
                        )
                        | None => StdlibNamespace {
                            members: StdlibMap::EMPTY,
                            shape: None,
                            deprecated: None,
                        },
                    };
                    StdlibEntry::Namespace(
                        self.allocator.alloc(StdlibNamespace {
                            members: self.members(
                                under.members,
                                &namespace.members,
                                &format!("{path}."),
                            )?,
                            shape: self
                                .shape_name(namespace.shape.as_ref(), &path)?
                                .or(under.shape),
                            deprecated: namespace
                                .deprecated
                                .as_ref()
                                .map(|deprecation| self.deprecation(deprecation))
                                .or(under.deprecated),
                        }),
                    )
                }
            };
            self.insert(&mut members, name, built);
        }
        self.map(members)
    }

    /// Puts `value` under `name`, keeping the name `entries` already holds.
    fn insert<V>(&self, entries: &mut BTreeMap<&'lib str, V>, name: &str, value: V) {
        if let Some(slot) = entries.get_mut(name) {
            *slot = value;
        } else {
            entries.insert(self.allocator.alloc_str(name), value);
        }
    }

    /// # Errors
    ///
    /// Fails when there are more entries than a map holds.
    fn map<V>(&self, entries: BTreeMap<&'lib str, V>) -> Result<StdlibMap<'lib, V>, Diagnostic>
    where
        V: Copy,
    {
        if entries.len() > MAX_MAP_ENTRIES {
            let problem = format!(
                "a table of the library would hold {} entries, past the {MAX_MAP_ENTRIES} it can",
                entries.len()
            );
            return Err(Diagnostic::e014(None, &problem));
        }
        let sorted: Vec<(&'lib str, V)> = entries.into_iter().collect();
        Ok(StdlibMap::new_in(self.allocator, &sorted))
    }

    /// # Errors
    ///
    /// Fails when the function, at `path`, names a shape or a constant set
    /// no library declares.
    fn function(
        &self,
        function: &Function,
        path: &str,
    ) -> Result<StdlibFunction<'lib>, Diagnostic> {
        let mut signatures = Vec::with_capacity(function.signatures.len());
        for params in &function.signatures {
            let mut built = Vec::with_capacity(params.len());
            for param in params {
                let kind = match &param.kind {
                    ArgKind::Any => StdlibArgKind::Any,
                    ArgKind::Bool => StdlibArgKind::Bool,
                    ArgKind::Number => StdlibArgKind::Number,
                    ArgKind::String => StdlibArgKind::String,
                    ArgKind::Function => StdlibArgKind::Function,
                    ArgKind::Table => StdlibArgKind::Table,
                    ArgKind::Display(label) => {
                        StdlibArgKind::Display(self.allocator.alloc_str(label))
                    }
                    ArgKind::Constant(Constants::Values(values)) => {
                        StdlibArgKind::Constant(self.constants(values))
                    }
                    ArgKind::Constant(Constants::Set(name)) => self.constant_set(name, path)?,
                    ArgKind::Vararg => StdlibArgKind::Vararg,
                };
                built.push(StdlibParam {
                    kind,
                    is_required: param.is_required,
                    deprecated: param
                        .deprecated
                        .as_ref()
                        .map(|deprecation| self.deprecation(deprecation)),
                });
            }
            signatures.push(StdlibSignature::new(self.allocator.alloc_slice_copy(&built)));
        }
        Ok(StdlibFunction {
            signatures: self.allocator.alloc_slice_copy(&signatures),
            is_pure: function.is_pure,
            is_must_use: function.is_must_use,
            deprecated: function
                .deprecated
                .as_ref()
                .map(|deprecation| self.deprecation(deprecation)),
            is_read_only: function.is_read_only,
            is_method: function.is_method,
            returns_shape: self.shape_name(function.returns_shape.as_ref(), path)?,
        })
    }

    /// What a param at `path` that takes the constant set `name` takes.
    ///
    /// # Errors
    ///
    /// Fails when no library declares the set.
    fn constant_set(
        &self,
        name: &Reference,
        path: &str,
    ) -> Result<StdlibArgKind<'lib>, Diagnostic> {
        let set = name.get_ref();
        if let Some(values) = self.constant_sets.get(set) {
            return Ok(StdlibArgKind::Constant(values));
        }
        if LuaTarget::ALL
            .into_iter()
            .any(|target| library_for(target).constant_sets.contains_key(set))
        {
            return Ok(StdlibArgKind::String);
        }
        Err(Diagnostic::e014(
            Some(place(name)),
            &format!("`{path}` names the constant set `{set}`, which no library declares"),
        ))
    }

    /// `name`, a shape `path` names, checked against every shape a library
    /// declares.
    ///
    /// # Errors
    ///
    /// Fails when no library declares the shape.
    fn shape_name(
        &self,
        name: Option<&Reference>,
        path: &str,
    ) -> Result<Option<&'lib str>, Diagnostic> {
        let Some(reference) = name else {
            return Ok(None);
        };
        let shape = reference.get_ref();
        let is_declared = self.definition.shapes().contains_key(shape)
            || self.base.shapes.contains_key(shape)
            || LuaTarget::ALL
                .into_iter()
                .any(|target| library_for(target).shapes.contains_key(shape));
        if !is_declared {
            return Err(Diagnostic::e014(
                Some(place(reference)),
                &format!("`{path}` names the shape `{shape}`, which no library declares"),
            ));
        }
        Ok(Some(self.allocator.alloc_str(shape)))
    }

    /// # Errors
    ///
    /// Fails when the value, at `path`, names a shape no library declares.
    fn value(&self, value: &Value, path: &str) -> Result<StdlibValue<'lib>, Diagnostic> {
        Ok(StdlibValue {
            is_read_only: value.is_read_only,
            deprecated: value.deprecated.as_ref().map(|deprecation| self.deprecation(deprecation)),
            shape: self.shape_name(value.shape.as_ref(), path)?,
        })
    }

    fn constants(&self, values: &[Constant]) -> &'lib [StdlibConstant<'lib>] {
        let built: Vec<StdlibConstant<'lib>> = values
            .iter()
            .map(|constant| StdlibConstant {
                value: self.allocator.alloc_str(&constant.value),
                deprecated: constant
                    .deprecated
                    .as_ref()
                    .map(|deprecation| self.deprecation(deprecation)),
                is_common: constant.is_common,
            })
            .collect();
        self.allocator.alloc_slice_copy(&built)
    }

    fn deprecation(&self, deprecation: &Deprecation) -> StdlibDeprecation<'lib> {
        StdlibDeprecation {
            message: self.allocator.alloc_str(&deprecation.message),
            replacement: deprecation.replacement.as_ref().map(|replacement| StdlibReplacement {
                template: self.allocator.alloc_str(&replacement.template),
                is_exact: replacement.is_exact,
            }),
        }
    }
}
