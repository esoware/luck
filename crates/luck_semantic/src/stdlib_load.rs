//! Loads the stdlib catalog: `Raw*` types mirror the TOML schema and
//! convert into the [`StdlibLibrary`] model.
//!
//! Loading a library also merges namespaces split across its files,
//! splices in the enum tree, resolves shape `extends`, and derives the
//! `string` receiver shape.
//!
//! TOML layout:
//! ```toml
//!   [globals.print]
//!   kind = "function"
//!   min_args = 0
//!   max_args = -1            # -1 = unbounded
//!   params = [{ kind = "vararg", required = false }]
//!
//!   [globals.load]           # overloaded: one table per signature
//!   kind = "function"
//!   overloads = [
//!     { min_args = 1, max_args = 2, params = [...] },
//!     { min_args = 1, max_args = 4, params = [...] },
//!   ]
//!
//!   [globals.string]
//!   kind = "namespace"
//!   [globals.string.members.byte]
//!   kind = "function"
//!   ...
//!
//!   [shapes.file.members.read]  # colon-method on the `file` shape
//!   kind = "function"
//!   method = true
//!   ...
//! ```

#![expect(
    clippy::panic,
    reason = "the catalog is built in, so a bad entry is a luck bug, not user input"
)]

use alloc::collections::BTreeMap;
use core::mem;

use luck_token::{CompactString, LuaVersion, StdlibEnvironment};
use serde::Deserialize;

use crate::stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibNamespace, StdlibParam, StdlibShape, StdlibSignature, StdlibValue,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLibrary {
    #[serde(default)]
    globals: BTreeMap<String, RawEntry>,
    #[serde(default)]
    shapes: BTreeMap<String, RawShape>,
    /// Named value lists that a param in any of the library's files can
    /// use with `{ kind = "constant", set = "<name>" }`, so the large
    /// generated Roblox service and class lists are written once.
    #[serde(default)]
    constant_sets: BTreeMap<String, RawConstantSet>,
    /// The generated Roblox enum tree. Each `[enums.<Type>]` becomes an
    /// `Enum`-shaped namespace under the `Enum` global, with its items as
    /// read-only `EnumItem`-shaped constants. The `Enum` global itself must
    /// be hand-authored.
    #[serde(default)]
    enums: BTreeMap<String, RawEnumType>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnumType {
    items: Vec<RawConstantValue>,
    #[serde(default)]
    deprecated: Option<RawDeprecation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConstantSet {
    values: Vec<RawConstantValue>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShape {
    /// Base shape whose members are copied in at load; own members win.
    /// Lets Roblox shapes share `Instance`'s members without modeling the
    /// class hierarchy.
    #[serde(default)]
    extends: Option<String>,
    #[serde(default)]
    members: BTreeMap<String, RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RawEntry {
    Function(RawFunction),
    Constant(RawValue),
    Property(RawValue),
    Namespace(RawNamespace),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the catalog TOML, and all sixteen combinations are legal"
)]
struct RawFunction {
    // Flat single-signature form; mutually exclusive with `overloads`.
    #[serde(default)]
    params: Option<Vec<RawParam>>,
    #[serde(default)]
    min_args: Option<usize>,
    /// Negative (by convention `-1`) means unbounded, since TOML has no
    /// null.
    #[serde(default)]
    max_args: Option<i32>,
    #[serde(default)]
    overloads: Vec<RawSignature>,
    #[serde(default)]
    is_pure: bool,
    #[serde(default)]
    must_use: bool,
    #[serde(default)]
    deprecated: Option<RawDeprecation>,
    #[serde(default = "default_true")]
    read_only: bool,
    #[serde(default)]
    method: bool,
    #[serde(default)]
    returns: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSignature {
    #[serde(default)]
    params: Vec<RawParam>,
    min_args: usize,
    max_args: i32,
}

// No `deny_unknown_fields`: serde can't combine it with the flattened kind
// tag.
#[derive(Debug, Deserialize)]
struct RawParam {
    #[serde(flatten)]
    kind: RawParamKind,
    #[serde(default = "default_true")]
    required: bool,
    #[serde(default)]
    deprecated: Option<RawDeprecation>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RawParamKind {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    Display {
        display: String,
    },
    Constant {
        #[serde(default)]
        values: Vec<RawConstantValue>,
        /// Reference to a `[constant_sets.<name>]` list; mutually
        /// exclusive with inline `values`.
        #[serde(default)]
        set: Option<String>,
    },
    Vararg,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawConstantValue {
    Plain(String),
    Detailed {
        value: String,
        #[serde(default)]
        deprecated: Option<RawDeprecation>,
        #[serde(default)]
        common: bool,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawValue {
    #[serde(default = "default_true")]
    read_only: bool,
    #[serde(default)]
    deprecated: Option<RawDeprecation>,
    #[serde(default)]
    shape: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeprecation {
    message: String,
    #[serde(default)]
    replace_template: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNamespace {
    #[serde(default)]
    members: BTreeMap<String, RawEntry>,
    #[serde(default)]
    shape: Option<String>,
    #[serde(default)]
    deprecated: Option<RawDeprecation>,
}

const fn default_true() -> bool {
    true
}

/// Converted constant sets from all of a library's files, which
/// `set = "<name>"` params clone from.
type ConstantSets = BTreeMap<CompactString, Vec<StdlibConstant>>;

impl From<RawConstantValue> for StdlibConstant {
    fn from(raw: RawConstantValue) -> Self {
        match raw {
            RawConstantValue::Plain(value) => {
                Self { value: value.into(), deprecated: None, is_common: false }
            }
            RawConstantValue::Detailed { value, deprecated, common } => Self {
                value: value.into(),
                deprecated: deprecated.map(StdlibDeprecation::from),
                is_common: common,
            },
        }
    }
}

impl From<RawDeprecation> for StdlibDeprecation {
    fn from(raw: RawDeprecation) -> Self {
        Self {
            message: raw.message.into(),
            replace_template: raw.replace_template.map(CompactString::from),
        }
    }
}

impl From<RawValue> for StdlibValue {
    fn from(raw: RawValue) -> Self {
        Self {
            is_read_only: raw.read_only,
            deprecated: raw.deprecated.map(StdlibDeprecation::from),
            shape: raw.shape.map(CompactString::from),
        }
    }
}

/// # Panics
///
/// Panics on a constant param that names an unknown set, or that has both
/// or neither of inline values and a set reference.
fn convert_param(raw: RawParam, sets: &ConstantSets, path: &str) -> StdlibParam {
    let kind = match raw.kind {
        RawParamKind::Any => StdlibArgKind::Any,
        RawParamKind::Bool => StdlibArgKind::Bool,
        RawParamKind::Number => StdlibArgKind::Number,
        RawParamKind::String => StdlibArgKind::String,
        RawParamKind::Function => StdlibArgKind::Function,
        RawParamKind::Table => StdlibArgKind::Table,
        RawParamKind::Display { display } => StdlibArgKind::Display(display.into()),
        RawParamKind::Constant { values, set } => {
            let constants = match (values.is_empty(), set) {
                (false, None) => values.into_iter().map(StdlibConstant::from).collect(),
                (true, Some(set_name)) => sets
                    .get(set_name.as_str())
                    .unwrap_or_else(|| {
                        panic!("stdlib entry {path}: unknown constant set '{set_name}'")
                    })
                    .clone(),
                (false, Some(_)) | (true, None) => panic!(
                    "stdlib entry {path}: constant param needs inline values or a set \
                     reference, but not both"
                ),
            };
            StdlibArgKind::Constant(constants)
        }
        RawParamKind::Vararg => StdlibArgKind::Vararg,
    };
    StdlibParam {
        kind,
        is_required: raw.required,
        deprecated: raw.deprecated.map(StdlibDeprecation::from),
    }
}

fn convert_signature(
    params: Vec<RawParam>,
    min_args: usize,
    max_args: i32,
    sets: &ConstantSets,
    path: &str,
) -> StdlibSignature {
    StdlibSignature {
        params: params.into_iter().map(|param| convert_param(param, sets, path)).collect(),
        min_args,
        max_args: usize::try_from(max_args).ok(),
    }
}

/// # Panics
///
/// Panics when a function without overloads lacks `min_args` or
/// `max_args`, or when one with overloads also has flat signature fields.
fn convert_function(raw: RawFunction, path: &str, sets: &ConstantSets) -> StdlibFunction {
    let has_flat_signature =
        raw.min_args.is_some() || raw.max_args.is_some() || raw.params.is_some();
    let signatures: Vec<StdlibSignature> = if raw.overloads.is_empty() {
        let (Some(min_args), Some(max_args)) = (raw.min_args, raw.max_args) else {
            panic!("stdlib entry {path}: flat signature requires min_args and max_args");
        };
        vec![convert_signature(raw.params.unwrap_or_default(), min_args, max_args, sets, path)]
    } else {
        assert!(
            !has_flat_signature,
            "stdlib entry {path}: flat signature fields and overloads are mutually exclusive"
        );
        raw.overloads
            .into_iter()
            .map(|signature| {
                convert_signature(
                    signature.params,
                    signature.min_args,
                    signature.max_args,
                    sets,
                    path,
                )
            })
            .collect()
    };
    StdlibFunction {
        signatures,
        is_pure: raw.is_pure,
        is_must_use: raw.must_use,
        deprecated: raw.deprecated.map(StdlibDeprecation::from),
        is_read_only: raw.read_only,
        is_method: raw.method,
        returns_shape: raw.returns.map(CompactString::from),
    }
}

fn convert_entry(raw: RawEntry, path: &str, sets: &ConstantSets) -> StdlibEntry {
    match raw {
        RawEntry::Function(function) => {
            StdlibEntry::Function(Box::new(convert_function(function, path, sets)))
        }
        RawEntry::Constant(value) => StdlibEntry::Constant(value.into()),
        RawEntry::Property(value) => StdlibEntry::Property(value.into()),
        RawEntry::Namespace(namespace) => {
            let mut members = BTreeMap::new();
            for (name, child) in namespace.members {
                let child_path = format!("{path}.{name}");
                members.insert(name.into(), convert_entry(child, &child_path, sets));
            }
            StdlibEntry::Namespace(StdlibNamespace {
                members,
                shape: namespace.shape.map(CompactString::from),
                deprecated: namespace.deprecated.map(StdlibDeprecation::from),
            })
        }
    }
}

/// String namespace members whose first param isn't the subject string,
/// so they aren't methods: `string.char` takes bytes and `string.dump` a
/// function.
const NON_RECEIVER_STRING_MEMBERS: &[&str] = &["char", "dump"];

/// Members whose first return value is itself a string, so the derived
/// methods chain: `("x"):rep(2):upper()`.
const STRING_RETURNING_MEMBERS: &[&str] =
    &["format", "gsub", "lower", "pack", "rep", "reverse", "sub", "upper"];

/// Derive the `string` receiver shape from the string namespace.
///
/// Every subject-first member becomes a colon-method with the subject param
/// dropped, mirroring Lua's string metatable. Hand-authored shape members
/// win over derived ones.
///
/// # Panics
///
/// Panics when a subject-first member declares a signature with no params.
fn derive_string_shape(
    globals: &BTreeMap<CompactString, StdlibEntry>,
    shapes: &mut BTreeMap<CompactString, StdlibShape>,
) {
    let Some(StdlibEntry::Namespace(string_namespace)) = globals.get("string") else {
        return;
    };
    let shape = shapes
        .entry(CompactString::const_new("string"))
        .or_insert_with(|| StdlibShape { members: BTreeMap::new() });
    for (name, entry) in &string_namespace.members {
        if NON_RECEIVER_STRING_MEMBERS.contains(&name.as_str()) || shape.members.contains_key(name)
        {
            continue;
        }
        let StdlibEntry::Function(function) = entry else {
            continue;
        };
        let mut method = (**function).clone();
        for signature in &mut method.signatures {
            assert!(
                !signature.params.is_empty(),
                "string.{name}: receiver derivation requires a leading subject param"
            );
            signature.params.remove(0);
            signature.min_args = signature.min_args.saturating_sub(1);
            signature.max_args = signature.max_args.map(|max| max.saturating_sub(1));
        }
        method.is_method = true;
        // Replace templates number the subject `%1`, but a colon call
        // passes it as the receiver, so keep the message and drop the fix.
        if let Some(deprecation) = &mut method.deprecated {
            deprecation.replace_template = None;
        }
        if STRING_RETURNING_MEMBERS.contains(&name.as_str()) {
            method.returns_shape = Some(CompactString::const_new("string"));
        }
        shape.members.insert(name.clone(), StdlibEntry::Function(Box::new(method)));
    }
}

/// Deep-merge `incoming` into `existing`. Namespaces merge recursively;
/// any other collision is a catalog bug.
///
/// # Panics
///
/// Panics when two non-namespace entries collide or two namespaces declare
/// different shapes.
fn merge_entry(existing: &mut StdlibEntry, incoming: StdlibEntry, path: &str) {
    let (StdlibEntry::Namespace(existing_namespace), StdlibEntry::Namespace(incoming_namespace)) =
        (existing, incoming)
    else {
        panic!("internal stdlib data conflict at {path}: non-namespace entry defined twice");
    };
    for (name, child) in incoming_namespace.members {
        let child_path = format!("{path}.{name}");
        match existing_namespace.members.get_mut(&name) {
            Some(existing_child) => merge_entry(existing_child, child, &child_path),
            None => {
                existing_namespace.members.insert(name, child);
            }
        }
    }
    if let Some(shape) = incoming_namespace.shape {
        let is_conflicting = existing_namespace
            .shape
            .as_ref()
            .is_some_and(|existing_shape| *existing_shape != shape);
        assert!(!is_conflicting, "namespace {path}: conflicting shapes");
        existing_namespace.shape = Some(shape);
    }
    if existing_namespace.deprecated.is_none() {
        existing_namespace.deprecated = incoming_namespace.deprecated;
    }
}

/// # Panics
///
/// Panics when the embedded catalog is malformed or inconsistent: a TOML
/// error, a duplicate definition, or a dangling reference.
pub(crate) fn parse_library(
    sources: &[&str],
    version: LuaVersion,
    environment: StdlibEnvironment,
) -> StdlibLibrary {
    let mut raw_libraries: Vec<RawLibrary> = sources
        .iter()
        .map(|toml_source| {
            toml::from_str(toml_source).unwrap_or_else(|error| {
                panic!("internal stdlib TOML parse error for {version:?}/{environment:?}: {error}")
            })
        })
        .collect();
    // Collect constant sets first, since a param in one file can use a set
    // from another (the generated Roblox API file).
    let mut sets: ConstantSets = BTreeMap::new();
    let mut enums: BTreeMap<String, RawEnumType> = BTreeMap::new();
    for raw in &mut raw_libraries {
        for (name, set) in mem::take(&mut raw.constant_sets) {
            let converted: Vec<StdlibConstant> =
                set.values.into_iter().map(StdlibConstant::from).collect();
            assert!(
                sets.insert(CompactString::from(name.as_str()), converted).is_none(),
                "duplicate constant set {name}"
            );
        }
        for (name, enum_type) in mem::take(&mut raw.enums) {
            assert!(enums.insert(name.clone(), enum_type).is_none(), "duplicate enum type {name}");
        }
    }
    let mut globals: BTreeMap<CompactString, StdlibEntry> = BTreeMap::new();
    let mut shapes: BTreeMap<CompactString, StdlibShape> = BTreeMap::new();
    let mut extends: BTreeMap<CompactString, CompactString> = BTreeMap::new();
    for raw in raw_libraries {
        for (name, entry) in raw.globals {
            let converted = convert_entry(entry, &name, &sets);
            match globals.get_mut(name.as_str()) {
                Some(existing) => merge_entry(existing, converted, &name),
                None => {
                    globals.insert(name.into(), converted);
                }
            }
        }
        for (shape_name, raw_shape) in raw.shapes {
            if let Some(base) = raw_shape.extends {
                let previous = extends.insert(
                    CompactString::from(shape_name.as_str()),
                    CompactString::from(base.as_str()),
                );
                assert!(
                    previous.is_none_or(|previous_base| previous_base == base),
                    "shape {shape_name}: conflicting extends declarations"
                );
            }
            let shape = shapes
                .entry(CompactString::from(shape_name.as_str()))
                .or_insert_with(|| StdlibShape { members: BTreeMap::new() });
            for (name, entry) in raw_shape.members {
                let path = format!("shape {shape_name}.{name}");
                let converted = convert_entry(entry, &path, &sets);
                match shape.members.get_mut(name.as_str()) {
                    Some(existing) => merge_entry(existing, converted, &path),
                    None => {
                        shape.members.insert(name.into(), converted);
                    }
                }
            }
        }
    }
    splice_enums(enums, &mut globals);
    resolve_shape_extends(&mut shapes, &extends);
    derive_string_shape(&globals, &mut shapes);
    let library = StdlibLibrary { version, environment, globals, shapes };
    verify_shape_references(&library);
    library
}

/// Expand the compact `[enums.<Type>]` section into member namespaces
/// of the `Enum` global.
///
/// Each type is an `Enum`-shaped namespace, each item an `EnumItem`-shaped
/// read-only constant. The `Enum` global and the `Enums`/`Enum`/`EnumItem`
/// shapes are hand-authored; only the tree of names is generated.
///
/// # Panics
///
/// Panics when the `Enum` global is absent or a type collides with one of
/// its hand-authored members.
fn splice_enums(
    enums: BTreeMap<String, RawEnumType>,
    globals: &mut BTreeMap<CompactString, StdlibEntry>,
) {
    if enums.is_empty() {
        return;
    }
    let Some(StdlibEntry::Namespace(enum_global)) = globals.get_mut("Enum") else {
        panic!("enum data requires a hand-authored Enum global namespace");
    };
    for (type_name, enum_type) in enums {
        let mut members: BTreeMap<CompactString, StdlibEntry> = BTreeMap::new();
        for item in enum_type.items {
            let constant = StdlibConstant::from(item);
            members.insert(
                constant.value,
                StdlibEntry::Constant(StdlibValue {
                    is_read_only: true,
                    deprecated: constant.deprecated,
                    shape: Some(CompactString::const_new("EnumItem")),
                }),
            );
        }
        let entry = StdlibEntry::Namespace(StdlibNamespace {
            members,
            shape: Some(CompactString::const_new("Enum")),
            deprecated: enum_type.deprecated.map(StdlibDeprecation::from),
        });
        assert!(
            enum_global.members.insert(CompactString::from(type_name.as_str()), entry).is_none(),
            "enum type {type_name} collides with a hand-authored Enum member"
        );
    }
}

/// Copy each extending shape's base members into it; own members win.
/// Chains resolve transitively, and a cycle panics.
fn resolve_shape_extends(
    shapes: &mut BTreeMap<CompactString, StdlibShape>,
    extends: &BTreeMap<CompactString, CompactString>,
) {
    /// # Panics
    ///
    /// Panics on an `extends` cycle or an unknown base shape.
    fn collect(
        name: &str,
        shapes: &BTreeMap<CompactString, StdlibShape>,
        extends: &BTreeMap<CompactString, CompactString>,
        chain: &mut Vec<CompactString>,
    ) -> BTreeMap<CompactString, StdlibEntry> {
        assert!(!chain.iter().any(|visited| visited == name), "shape extends cycle through {name}");
        chain.push(CompactString::from(name));
        let mut members = extends.get(name).map_or_else(BTreeMap::new, |base| {
            assert!(
                shapes.contains_key(base.as_str()),
                "shape {name} extends unknown shape {base}"
            );
            collect(base, shapes, extends, chain)
        });
        if let Some(shape) = shapes.get(name) {
            for (member_name, entry) in &shape.members {
                members.insert(member_name.clone(), entry.clone());
            }
        }
        chain.pop();
        members
    }
    for name in extends.keys() {
        let members = collect(name, shapes, extends, &mut Vec::new());
        shapes.insert(name.clone(), StdlibShape { members });
    }
}

/// Check that every `returns` and `shape` names a shape the library has.
fn verify_shape_references(library: &StdlibLibrary) {
    /// # Panics
    ///
    /// Panics when `entry` or a child names a shape the library lacks.
    fn check(library: &StdlibLibrary, entry: &StdlibEntry, path: &str) {
        let referenced = match entry {
            StdlibEntry::Function(function) => function.returns_shape.as_ref(),
            StdlibEntry::Constant(value) | StdlibEntry::Property(value) => value.shape.as_ref(),
            StdlibEntry::Namespace(namespace) => {
                for (name, child) in &namespace.members {
                    check(library, child, &format!("{path}.{name}"));
                }
                namespace.shape.as_ref()
            }
        };
        if let Some(shape) = referenced {
            assert!(
                library.shapes.contains_key(shape),
                "stdlib entry {path} references unknown shape {shape}"
            );
        }
    }
    for (name, entry) in &library.globals {
        check(library, entry, name);
    }
    for (shape_name, shape) in &library.shapes {
        for (name, entry) in &shape.members {
            check(library, entry, &format!("shape {shape_name}.{name}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_test_library(toml_source: &str) -> StdlibLibrary {
        parse_library(&[toml_source], LuaVersion::Luau, StdlibEnvironment::Standalone)
    }

    #[test]
    fn flat_signature_parses() {
        let library = parse_test_library(
            r#"
            [globals.f]
            kind = "function"
            min_args = 1
            max_args = 2
            params = [{ kind = "string" }, { kind = "number", required = false }]
            "#,
        );
        let StdlibEntry::Function(function) = library.lookup(&["f"]).expect("f") else {
            panic!("expected function");
        };
        assert_eq!(function.signatures.len(), 1);
        assert_eq!(function.min_args(), 1);
        assert_eq!(function.max_args(), Some(2));
    }

    #[test]
    fn overloads_parse_and_match() {
        let library = parse_test_library(
            r#"
            [globals.f]
            kind = "function"
            overloads = [
              { min_args = 0, max_args = 0, params = [] },
              { min_args = 2, max_args = 3, params = [
                  { kind = "number" },
                  { kind = "number" },
                  { kind = "number", required = false },
              ] },
            ]
            "#,
        );
        let StdlibEntry::Function(function) = library.lookup(&["f"]).expect("f") else {
            panic!("expected function");
        };
        assert_eq!(function.signatures.len(), 2);
        assert_eq!(function.min_args(), 0);
        assert_eq!(function.max_args(), Some(3));
        assert!(function.accepts_arg_count(0));
        assert!(!function.accepts_arg_count(1));
        assert!(function.accepts_arg_count(2));
        assert!(function.accepts_arg_count(3));
        assert!(!function.accepts_arg_count(4));
        assert_eq!(function.matching_signatures(2).count(), 1);
        assert_eq!(function.signature_index_for_active_param(1), 1);
    }

    #[test]
    fn per_constant_deprecation_parses() {
        let library = parse_test_library(
            r#"
            [globals.gc]
            kind = "function"
            min_args = 0
            max_args = 1
            params = [{ kind = "constant", required = false, values = [
              "collect",
              { value = "setpause", deprecated = { message = "gone" } },
            ] }]
            "#,
        );
        let StdlibEntry::Function(function) = library.lookup(&["gc"]).expect("gc") else {
            panic!("expected function");
        };
        let StdlibArgKind::Constant(values) = &function.primary_signature().params[0].kind else {
            panic!("expected constant param");
        };
        assert_eq!(values.len(), 2);
        assert!(values[0].deprecated.is_none());
        assert_eq!(
            values[1].deprecated.as_ref().map(|deprecation| deprecation.message.as_str()),
            Some("gone")
        );
    }

    #[test]
    fn shapes_resolve_through_properties() {
        let library = parse_test_library(
            r#"
            [globals.game]
            kind = "property"
            shape = "DataModel"

            [shapes.DataModel.members.GetService]
            kind = "function"
            method = true
            must_use = true
            min_args = 1
            max_args = 1
            params = [{ kind = "string" }]
            returns = "Instance"

            [shapes.Instance.members.FindFirstChild]
            kind = "function"
            method = true
            min_args = 1
            max_args = 2
            params = [{ kind = "string" }, { kind = "bool", required = false }]
            "#,
        );
        let entry = library.lookup(&["game", "GetService"]).expect("shape traversal");
        let StdlibEntry::Function(function) = entry else {
            panic!("expected function");
        };
        assert!(function.is_method);
        assert_eq!(function.returns_shape.as_deref(), Some("Instance"));
        assert!(library.shape_member("Instance", "FindFirstChild").is_some());
    }

    #[test]
    fn per_param_deprecation_parses() {
        let library = parse_test_library(
            r#"
            [globals.f]
            kind = "function"
            min_args = 1
            max_args = 2
            params = [
              { kind = "string" },
              { kind = "any", required = false, deprecated = { message = "use x instead" } },
            ]
            "#,
        );
        let StdlibEntry::Function(function) = library.lookup(&["f"]).expect("f") else {
            panic!("expected function");
        };
        assert!(function.primary_signature().params[1].deprecated.is_some());
    }

    #[test]
    fn splice_merges_namespaces_and_shapes() {
        let base = r#"
            [globals.Enum]
            kind = "namespace"
            [globals.Enum.members.Material]
            kind = "namespace"
            [globals.Enum.members.Material.members.Grass]
            kind = "constant"
        "#;
        // A spliced file redeclares the namespace header it extends.
        let extra = r#"
            [globals.Enum]
            kind = "namespace"
            [globals.Enum.members.KeyCode]
            kind = "namespace"
            [globals.Enum.members.KeyCode.members.A]
            kind = "constant"

            [shapes.Instance.members.GetChildren]
            kind = "function"
            method = true
            min_args = 0
            max_args = 0
        "#;
        let library = parse_library(&[base, extra], LuaVersion::Luau, StdlibEnvironment::Roblox);
        assert!(library.lookup(&["Enum", "Material", "Grass"]).is_some());
        assert!(library.lookup(&["Enum", "KeyCode", "A"]).is_some());
        assert!(library.shape_member("Instance", "GetChildren").is_some());
    }

    #[test]
    #[should_panic(expected = "references unknown shape")]
    fn dangling_shape_reference_panics() {
        parse_test_library(
            r#"
            [globals.game]
            kind = "property"
            shape = "Nope"
            "#,
        );
    }
}
