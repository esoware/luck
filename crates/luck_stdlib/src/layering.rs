use std::collections::BTreeMap;

use luck_allocator::Allocator;
use luck_diagnostics::Diagnostic;

use crate::stdlib_catalog::library_for;
use crate::stdlib_definition::{
    ArgKind, Constant, Constants, Deprecation, Entry, Function, Reference, StdlibDefinition, Value,
    place,
};
use crate::stdlib_map::{MAX_MAP_ENTRIES, StdlibMap};
use crate::stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibNamespace, StdlibParam, StdlibReplacement, StdlibShape, StdlibSignature, StdlibValue,
};
use crate::variant::Variant;

/// `definition` layered over `base`, as [`Stdlib::layer`](crate::Stdlib::layer)
/// describes, in `allocator`.
pub(crate) fn layer<'lib>(
    allocator: &'lib Allocator,
    base: &StdlibLibrary<'lib>,
    definition: &StdlibDefinition,
) -> Result<StdlibLibrary<'lib>, Diagnostic> {
    let mut layering = Layering { allocator, base, definition, constant_sets: base.constant_sets };
    let constant_sets =
        layering.layered(base.constant_sets, &definition.constant_sets, |_, values, _| {
            Ok(layering.constants(values))
        })?;
    layering.constant_sets = layering.map(constant_sets)?;
    let shapes = layering.layered(base.shapes, &definition.shapes, |name, shape, below| {
        let below = below.unwrap_or(StdlibShape { members: StdlibMap::EMPTY, extends: None });
        Ok(StdlibShape {
            members: layering.members(below.members, &shape.members, &format!("{name}:"))?,
            extends: layering.shape_name(shape.extends.as_ref(), name)?.or(below.extends),
        })
    })?;
    Ok(StdlibLibrary {
        variant: base.variant,
        globals: layering.members(base.globals, &definition.globals, "")?,
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
    /// `below`'s entries with `layer`'s over them, each of which `make`
    /// makes from its name, what the definition declares, and the entry it
    /// replaces.
    fn layered<V, Declared>(
        &self,
        below: StdlibMap<'lib, V>,
        layer: &BTreeMap<String, Declared>,
        mut make: impl FnMut(&str, &Declared, Option<V>) -> Result<V, Diagnostic>,
    ) -> Result<BTreeMap<&'lib str, V>, Diagnostic>
    where
        V: Copy,
    {
        let mut entries: BTreeMap<&'lib str, V> =
            below.iter().map(|(name, value)| (name, *value)).collect();
        for (name, declared) in layer {
            let entry = make(name, declared, entries.get(name.as_str()).copied())?;
            if let Some(slot) = entries.get_mut(name.as_str()) {
                *slot = entry;
            } else {
                entries.insert(self.allocator.alloc_str(name), entry);
            }
        }
        Ok(entries)
    }

    /// `below`'s members with `layer`'s over them, where `prefix` begins the
    /// path of each.
    fn members(
        &self,
        below: StdlibMap<'lib, StdlibEntry<'lib>>,
        layer: &BTreeMap<String, Entry>,
        prefix: &str,
    ) -> Result<StdlibMap<'lib, StdlibEntry<'lib>>, Diagnostic> {
        let members = self.layered(below, layer, |name, entry, replaced| {
            let path = format!("{prefix}{name}");
            Ok(match entry {
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
                    let under = match replaced {
                        Some(StdlibEntry::Namespace(found)) => *found,
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
            })
        })?;
        self.map(members)
    }

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
    fn constant_set(
        &self,
        name: &Reference,
        path: &str,
    ) -> Result<StdlibArgKind<'lib>, Diagnostic> {
        let set = name.get_ref();
        if let Some(values) = self.constant_sets.get(set) {
            return Ok(StdlibArgKind::Constant(values));
        }
        // Roblox's library is Luau's with more layered over it, so it holds
        // every set and shape a built-in library declares.
        if library_for(Variant::Roblox).constant_sets.contains_key(set) {
            return Ok(StdlibArgKind::String);
        }
        Err(Diagnostic::e014(
            Some(place(name)),
            &format!("`{path}` names the constant set `{set}`, which no library declares"),
        ))
    }

    /// `name`, a shape `path` names, checked against every shape a library
    /// declares.
    fn shape_name(
        &self,
        name: Option<&Reference>,
        path: &str,
    ) -> Result<Option<&'lib str>, Diagnostic> {
        let Some(reference) = name else {
            return Ok(None);
        };
        let shape = reference.get_ref();
        let is_declared = self.definition.shapes.contains_key(shape)
            || self.base.shapes.contains_key(shape)
            || library_for(Variant::Roblox).shapes.contains_key(shape);
        if !is_declared {
            return Err(Diagnostic::e014(
                Some(place(reference)),
                &format!("`{path}` names the shape `{shape}`, which no library declares"),
            ));
        }
        Ok(Some(self.allocator.alloc_str(shape)))
    }

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
