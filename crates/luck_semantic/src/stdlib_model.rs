//! Rich stdlib data model: overloaded signatures, typed argument
//! metadata, colon-method entries on named shapes, and deprecation at
//! the entry, parameter, and constant-value level.
//!
//! Each supported environment ships as one fully self-contained TOML
//! file under `stdlib_data/`: `lua51`..`lua55`, `luau` (standalone
//! Luau), and `luau_roblox` (the Roblox runtime). The files are
//! deliberately independent - no inheritance or tier layering - because
//! the environments diverge in both directions; shared entries are
//! duplicated and kept honest by drift-guard tests. A library is
//! selected by `(LuaVersion, StdlibEnvironment)`; the environment only
//! matters for Luau. Data is parsed once via `include_str!` +
//! `LazyLock`; extra generated sources (services, enums) splice into a
//! library's file list at load.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use compact_str::CompactString;
use luck_token::{LuaVersion, StdlibEnvironment};
use serde::Deserialize;

/// A single entry: function, constant, namespace, or property.
#[derive(Debug, Clone)]
pub enum StdlibEntry {
    Function(Box<StdlibFunction>),
    Constant(StdlibValue),
    Property(StdlibValue),
    Namespace(StdlibNamespace),
}

impl StdlibEntry {
    /// Entry-level deprecation, regardless of entry kind.
    #[must_use]
    pub fn deprecation(&self) -> Option<&StdlibDeprecation> {
        match self {
            StdlibEntry::Function(func) => func.deprecated.as_ref(),
            StdlibEntry::Constant(value) | StdlibEntry::Property(value) => {
                value.deprecated.as_ref()
            }
            StdlibEntry::Namespace(namespace) => namespace.deprecated.as_ref(),
        }
    }

    /// Whether rebinding this global is an error. Namespaces are always
    /// read-only at the identity level.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        match self {
            StdlibEntry::Function(func) => func.read_only,
            StdlibEntry::Constant(value) | StdlibEntry::Property(value) => value.read_only,
            StdlibEntry::Namespace(_) => true,
        }
    }
}

/// A stdlib function: one or more concrete signatures plus call-site
/// metadata. Boxed inside `StdlibEntry` to keep the enum tight.
#[derive(Debug, Clone)]
pub struct StdlibFunction {
    /// At least one; authoring order. The first is the primary form
    /// used where a single rendering is needed.
    pub signatures: Vec<StdlibSignature>,
    pub is_pure: bool,
    pub must_use: bool,
    pub deprecated: Option<StdlibDeprecation>,
    pub read_only: bool,
    /// Colon-callable on its owning shape; `self` is excluded from
    /// every signature's params.
    pub is_method: bool,
    /// Shape of the call's first return value, when it names a shape in
    /// the library's registry (e.g. `io.open` returns `file`).
    pub returns_shape: Option<CompactString>,
}

impl StdlibFunction {
    /// Minimum arity across all signatures.
    #[must_use]
    pub fn min_args(&self) -> usize {
        self.signatures
            .iter()
            .map(|sig| sig.min_args)
            .min()
            .unwrap_or(0)
    }

    /// Maximum arity across all signatures; `None` if any is unbounded.
    #[must_use]
    pub fn max_args(&self) -> Option<usize> {
        let mut max = 0;
        for sig in &self.signatures {
            match sig.max_args {
                Some(n) => max = max.max(n),
                None => return None,
            }
        }
        Some(max)
    }

    /// Whether any signature accepts a call with `count` arguments.
    #[must_use]
    pub fn accepts_arg_count(&self, count: usize) -> bool {
        self.signatures.iter().any(|sig| sig.accepts(count))
    }

    /// Signatures that accept a call with `count` arguments.
    pub fn matching_signatures(&self, count: usize) -> impl Iterator<Item = &StdlibSignature> {
        self.signatures.iter().filter(move |sig| sig.accepts(count))
    }

    /// The primary (first) signature.
    #[must_use]
    pub fn primary_signature(&self) -> &StdlibSignature {
        &self.signatures[0]
    }

    /// Index of the signature to highlight when the cursor sits on
    /// parameter `active_param`: the first signature with a slot at
    /// that position, else the last.
    #[must_use]
    pub fn signature_index_for_active_param(&self, active_param: usize) -> usize {
        self.signatures
            .iter()
            .position(|sig| {
                active_param < sig.params.len()
                    || sig
                        .params
                        .last()
                        .is_some_and(|param| matches!(param.kind, StdlibArgKind::Vararg))
            })
            .unwrap_or(self.signatures.len() - 1)
    }
}

/// One concrete signature of a function.
#[derive(Debug, Clone)]
pub struct StdlibSignature {
    pub params: Vec<StdlibParam>,
    pub min_args: usize,
    /// `None` = unbounded (trailing vararg).
    pub max_args: Option<usize>,
}

impl StdlibSignature {
    #[must_use]
    pub fn accepts(&self, count: usize) -> bool {
        count >= self.min_args && self.max_args.is_none_or(|max| count <= max)
    }
}

#[derive(Debug, Clone)]
pub struct StdlibParam {
    pub kind: StdlibArgKind,
    pub required: bool,
    pub accepts_nil: bool,
    /// Set when passing this parameter at all is deprecated (e.g. the
    /// `parent` argument of `Instance.new`).
    pub deprecated: Option<StdlibDeprecation>,
}

#[derive(Debug, Clone)]
pub enum StdlibArgKind {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    /// A display-typed argument, e.g. "file" or "buffer". Treated as
    /// any-shape at the lint level; the string is the display label.
    Display(CompactString),
    Nil,
    /// Restricted set of literal string constants the argument must
    /// take. Used by e.g. `collectgarbage("collect")`.
    Constant(Vec<StdlibConstant>),
    /// `...` - variadic; matches zero or more values of any type.
    Vararg,
}

/// One allowed value of a `Constant`-kind parameter. Individual values
/// can be deprecated (e.g. `collectgarbage("setpause")` in 5.4).
#[derive(Debug, Clone)]
pub struct StdlibConstant {
    pub value: CompactString,
    pub deprecated: Option<StdlibDeprecation>,
    /// Curated frequently-used value; completion ranks these first
    /// (e.g. `Players` among the 300+ Roblox service names).
    pub is_common: bool,
}

#[derive(Debug, Clone)]
pub struct StdlibDeprecation {
    pub message: CompactString,
    /// Replacement template. Placeholders `%1`, `%2`, ... expand to the
    /// stringified positional arguments of the call.
    pub replace_template: Option<CompactString>,
}

#[derive(Debug, Clone)]
pub struct StdlibValue {
    pub read_only: bool,
    pub deprecated: Option<StdlibDeprecation>,
    /// Shape of the value, when it names a shape in the library's
    /// registry (e.g. `game` is a `DataModel`, `io.stdout` a `file`).
    pub shape: Option<CompactString>,
}

/// A dotted-member container. `shape` is the namespace's own value
/// shape, for namespaces that are also first-class values with methods
/// (each `Enum.<Type>` is an `Enum` value carrying `GetEnumItems`).
#[derive(Debug, Clone)]
pub struct StdlibNamespace {
    pub members: BTreeMap<CompactString, StdlibEntry>,
    pub shape: Option<CompactString>,
    pub deprecated: Option<StdlibDeprecation>,
}

/// A named value shape: the member surface of a non-global stdlib value
/// (file handles, the string receiver, Roblox instances). Members with
/// `is_method` are colon-callable; the rest are dot accesses.
#[derive(Debug, Clone)]
pub struct StdlibShape {
    pub members: BTreeMap<CompactString, StdlibEntry>,
}

/// Top-level library: the complete global surface of one environment.
#[derive(Debug, Clone)]
pub struct StdlibLibrary {
    pub version: LuaVersion,
    pub environment: StdlibEnvironment,
    pub globals: BTreeMap<CompactString, StdlibEntry>,
    pub shapes: BTreeMap<CompactString, StdlibShape>,
}

impl StdlibLibrary {
    /// Resolve a dotted path like `["table", "concat"]` to its entry.
    /// Traversal follows namespace members and the shape of a property
    /// or constant, so `["game", "GetService"]` resolves once `game`
    /// declares a shape.
    #[must_use]
    pub fn lookup(&self, path: &[CompactString]) -> Option<&StdlibEntry> {
        self.lookup_segments(path.iter().map(CompactString::as_str))
    }

    /// Same as [`Self::lookup`] but accepts borrowed `&str` segments -
    /// preferred entry point for callers that have raw source slices.
    #[must_use]
    pub fn lookup_str(&self, path: &[&str]) -> Option<&StdlibEntry> {
        self.lookup_segments(path.iter().copied())
    }

    fn lookup_segments<'lib, 'seg>(
        &'lib self,
        mut segments: impl Iterator<Item = &'seg str>,
    ) -> Option<&'lib StdlibEntry> {
        let mut current = self.globals.get(segments.next()?)?;
        for segment in segments {
            current = self.child(current, segment)?;
        }
        Some(current)
    }

    /// The member reachable from `entry` under `segment`: a namespace
    /// member, or a member of the entry's declared value shape.
    #[must_use]
    pub fn child<'lib>(
        &'lib self,
        entry: &'lib StdlibEntry,
        segment: &str,
    ) -> Option<&'lib StdlibEntry> {
        match entry {
            StdlibEntry::Namespace(namespace) => namespace.members.get(segment).or_else(|| {
                // A shaped namespace is also a value: `Enum.Material`
                // exposes its items AND the `Enum` shape's methods.
                self.shape_member(namespace.shape.as_ref()?, segment)
            }),
            StdlibEntry::Constant(value) | StdlibEntry::Property(value) => {
                self.shape_member(value.shape.as_ref()?, segment)
            }
            StdlibEntry::Function(_) => None,
        }
    }

    /// Member of a named shape.
    #[must_use]
    pub fn shape_member(&self, shape: &str, name: &str) -> Option<&StdlibEntry> {
        self.shapes.get(shape)?.members.get(name)
    }
}

// TOML layout:
//   [globals.print]
//   kind = "function"
//   min_args = 0
//   max_args = -1            # -1 = unbounded
//   params = [{ kind = "vararg", required = false }]
//
//   [globals.load]           # overloaded: one table per signature
//   kind = "function"
//   overloads = [
//     { min_args = 1, max_args = 2, params = [...] },
//     { min_args = 1, max_args = 4, params = [...] },
//   ]
//
//   [globals.string]
//   kind = "namespace"
//   [globals.string.members.byte]
//   kind = "function"
//   ...
//
//   [shapes.file.members.read]  # colon-method on the `file` shape
//   kind = "function"
//   method = true
//   ...

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLibrary {
    #[serde(default)]
    globals: BTreeMap<String, RawEntry>,
    #[serde(default)]
    shapes: BTreeMap<String, RawShape>,
    /// Named constant-value lists referencable from any source in the
    /// same library via `{ kind = "constant", set = "<name>" }`. Lets
    /// large generated sets (Roblox service and class names) be encoded
    /// once and shared by several parameters.
    #[serde(default)]
    constant_sets: BTreeMap<String, RawConstantSet>,
    /// Compact enum-tree section for the generated Roblox enum data:
    /// each `[enums.<Type>]` becomes a member namespace of the `Enum`
    /// global (shape `Enum`) whose items are read-only constants (shape
    /// `EnumItem`). Requires a hand-authored `Enum` global namespace.
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
    /// Mirrors the one inheritance level real Roblox instances share
    /// (everything is an Instance) without a class hierarchy.
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
struct RawFunction {
    // Flat single-signature form; mutually exclusive with `overloads`.
    #[serde(default)]
    params: Option<Vec<RawParam>>,
    #[serde(default)]
    min_args: Option<usize>,
    /// `-1` (or any negative) encodes "unbounded" - corresponds to
    /// `Option::None` in `StdlibSignature::max_args`. TOML has no
    /// native nullable so this sentinel keeps authoring ergonomic.
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

// No `deny_unknown_fields` here: serde cannot combine it with the
// flattened kind tag.
#[derive(Debug, Deserialize)]
struct RawParam {
    #[serde(flatten)]
    kind: RawParamKind,
    #[serde(default = "default_true")]
    required: bool,
    #[serde(default)]
    accepts_nil: bool,
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
    Nil,
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

/// Named constant sets shared across every source of a library, already
/// converted; `set = "<name>"` param references clone from here.
type ConstantSets = BTreeMap<CompactString, Vec<StdlibConstant>>;

fn convert_constant(value: RawConstantValue) -> StdlibConstant {
    match value {
        RawConstantValue::Plain(value) => StdlibConstant {
            value: value.into(),
            deprecated: None,
            is_common: false,
        },
        RawConstantValue::Detailed {
            value,
            deprecated,
            common,
        } => StdlibConstant {
            value: value.into(),
            deprecated: deprecated.map(convert_deprecation),
            is_common: common,
        },
    }
}

fn convert_param(raw: RawParam, sets: &ConstantSets, path: &str) -> StdlibParam {
    let kind = match raw.kind {
        RawParamKind::Any => StdlibArgKind::Any,
        RawParamKind::Bool => StdlibArgKind::Bool,
        RawParamKind::Number => StdlibArgKind::Number,
        RawParamKind::String => StdlibArgKind::String,
        RawParamKind::Function => StdlibArgKind::Function,
        RawParamKind::Table => StdlibArgKind::Table,
        RawParamKind::Display { display } => StdlibArgKind::Display(display.into()),
        RawParamKind::Nil => StdlibArgKind::Nil,
        RawParamKind::Constant { values, set } => {
            let constants = match (values.is_empty(), set) {
                (false, None) => values.into_iter().map(convert_constant).collect(),
                (true, Some(set_name)) => sets
                    .get(set_name.as_str())
                    .unwrap_or_else(|| {
                        panic!("stdlib entry {path}: unknown constant set '{set_name}'")
                    })
                    .clone(),
                _ => panic!(
                    "stdlib entry {path}: constant param takes either inline values or a set \
                     reference, exactly one"
                ),
            };
            StdlibArgKind::Constant(constants)
        }
        RawParamKind::Vararg => StdlibArgKind::Vararg,
    };
    StdlibParam {
        kind,
        required: raw.required,
        accepts_nil: raw.accepts_nil,
        deprecated: raw.deprecated.map(convert_deprecation),
    }
}

fn convert_deprecation(raw: RawDeprecation) -> StdlibDeprecation {
    StdlibDeprecation {
        message: raw.message.into(),
        replace_template: raw.replace_template.map(CompactString::from),
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
        params: params
            .into_iter()
            .map(|param| convert_param(param, sets, path))
            .collect(),
        min_args,
        max_args: if max_args < 0 {
            None
        } else {
            Some(max_args as usize)
        },
    }
}

fn convert_function(func: RawFunction, path: &str, sets: &ConstantSets) -> StdlibFunction {
    let has_flat = func.min_args.is_some() || func.max_args.is_some() || func.params.is_some();
    let signatures: Vec<StdlibSignature> = if func.overloads.is_empty() {
        let (Some(min_args), Some(max_args)) = (func.min_args, func.max_args) else {
            panic!("stdlib entry {path}: flat signature requires min_args and max_args");
        };
        vec![convert_signature(
            func.params.unwrap_or_default(),
            min_args,
            max_args,
            sets,
            path,
        )]
    } else {
        assert!(
            !has_flat,
            "stdlib entry {path}: flat signature fields and overloads are mutually exclusive"
        );
        func.overloads
            .into_iter()
            .map(|sig| convert_signature(sig.params, sig.min_args, sig.max_args, sets, path))
            .collect()
    };
    StdlibFunction {
        signatures,
        is_pure: func.is_pure,
        must_use: func.must_use,
        deprecated: func.deprecated.map(convert_deprecation),
        read_only: func.read_only,
        is_method: func.method,
        returns_shape: func.returns.map(CompactString::from),
    }
}

fn convert_entry(raw: RawEntry, path: &str, sets: &ConstantSets) -> StdlibEntry {
    match raw {
        RawEntry::Function(func) => {
            StdlibEntry::Function(Box::new(convert_function(func, path, sets)))
        }
        RawEntry::Constant(value) => StdlibEntry::Constant(StdlibValue {
            read_only: value.read_only,
            deprecated: value.deprecated.map(convert_deprecation),
            shape: value.shape.map(CompactString::from),
        }),
        RawEntry::Property(value) => StdlibEntry::Property(StdlibValue {
            read_only: value.read_only,
            deprecated: value.deprecated.map(convert_deprecation),
            shape: value.shape.map(CompactString::from),
        }),
        RawEntry::Namespace(namespace) => {
            let mut members = BTreeMap::new();
            for (name, child) in namespace.members {
                let child_path = format!("{path}.{name}");
                members.insert(name.into(), convert_entry(child, &child_path, sets));
            }
            StdlibEntry::Namespace(StdlibNamespace {
                members,
                shape: namespace.shape.map(CompactString::from),
                deprecated: namespace.deprecated.map(convert_deprecation),
            })
        }
    }
}

/// Members of the string namespace that do not take the subject string
/// as their first parameter and therefore are not colon-callable:
/// `string.char` takes byte values, `string.dump` a function.
const NON_RECEIVER_STRING_MEMBERS: &[&str] = &["char", "dump"];

/// Members whose first return value is itself a string, so the derived
/// methods chain: `("x"):rep(2):upper()`.
const STRING_RETURNING_MEMBERS: &[&str] = &[
    "format", "gsub", "lower", "pack", "rep", "reverse", "sub", "upper",
];

/// Derive the `string` receiver shape from the string namespace: every
/// subject-first member becomes a colon-method with the subject param
/// dropped, mirroring Lua's string metatable. Hand-authored shape
/// members win over derived ones.
fn derive_string_shape(
    globals: &BTreeMap<CompactString, StdlibEntry>,
    shapes: &mut BTreeMap<CompactString, StdlibShape>,
) {
    let Some(StdlibEntry::Namespace(string_namespace)) = globals.get("string") else {
        return;
    };
    let shape = shapes
        .entry(CompactString::const_new("string"))
        .or_insert_with(|| StdlibShape {
            members: BTreeMap::new(),
        });
    for (name, entry) in &string_namespace.members {
        if NON_RECEIVER_STRING_MEMBERS.contains(&name.as_str()) || shape.members.contains_key(name)
        {
            continue;
        }
        let StdlibEntry::Function(func) = entry else {
            continue;
        };
        let mut method = (**func).clone();
        for sig in &mut method.signatures {
            assert!(
                !sig.params.is_empty(),
                "string.{name}: receiver derivation requires a leading subject param"
            );
            sig.params.remove(0);
            sig.min_args = sig.min_args.saturating_sub(1);
            sig.max_args = sig.max_args.map(|max| max.saturating_sub(1));
        }
        method.is_method = true;
        // Replace templates are written for the free-function form,
        // where the subject is argument 1; on a colon call the subject
        // is the receiver, so the positions no longer line up. Keep the
        // message, drop the auto-fix.
        if let Some(deprecation) = &mut method.deprecated {
            deprecation.replace_template = None;
        }
        if STRING_RETURNING_MEMBERS.contains(&name.as_str()) {
            method.returns_shape = Some(CompactString::const_new("string"));
        }
        shape
            .members
            .insert(name.clone(), StdlibEntry::Function(Box::new(method)));
    }
}

/// Deep-merge `incoming` into `existing`. Namespaces union recursively;
/// any other collision is an internal data bug.
fn merge_entry(existing: &mut StdlibEntry, incoming: StdlibEntry, path: &str) {
    match (existing, incoming) {
        (
            StdlibEntry::Namespace(existing_namespace),
            StdlibEntry::Namespace(incoming_namespace),
        ) => {
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
                let conflicting = existing_namespace
                    .shape
                    .as_ref()
                    .is_some_and(|existing_shape| *existing_shape != shape);
                assert!(!conflicting, "namespace {path}: conflicting shapes");
                existing_namespace.shape = Some(shape);
            }
            if existing_namespace.deprecated.is_none() {
                existing_namespace.deprecated = incoming_namespace.deprecated;
            }
        }
        _ => panic!("internal stdlib data conflict at {path}: non-namespace entry defined twice"),
    }
}

pub(crate) fn parse_library(
    sources: &[&str],
    version: LuaVersion,
    environment: StdlibEnvironment,
) -> StdlibLibrary {
    // unwrap is acceptable here: these TOML files ship inside the
    // binary via include_str!. A parse failure means a bug in our
    // ship - not user input - so we'd rather fail loudly at first
    // call.
    let mut raws: Vec<RawLibrary> = sources
        .iter()
        .map(|toml_src| {
            toml::from_str(toml_src).unwrap_or_else(|err| {
                panic!("internal stdlib TOML parse error for {version:?}/{environment:?}: {err}")
            })
        })
        .collect();
    // Constant sets first: params in any source may reference sets
    // contributed by another (e.g. the generated Roblox API file).
    let mut sets: ConstantSets = BTreeMap::new();
    let mut enums: BTreeMap<String, RawEnumType> = BTreeMap::new();
    for raw in &mut raws {
        for (name, set) in std::mem::take(&mut raw.constant_sets) {
            let converted: Vec<StdlibConstant> =
                set.values.into_iter().map(convert_constant).collect();
            assert!(
                sets.insert(CompactString::from(name.as_str()), converted)
                    .is_none(),
                "duplicate constant set {name}"
            );
        }
        for (name, enum_type) in std::mem::take(&mut raw.enums) {
            assert!(
                enums.insert(name.clone(), enum_type).is_none(),
                "duplicate enum type {name}"
            );
        }
    }
    let mut globals: BTreeMap<CompactString, StdlibEntry> = BTreeMap::new();
    let mut shapes: BTreeMap<CompactString, StdlibShape> = BTreeMap::new();
    let mut extends: BTreeMap<CompactString, CompactString> = BTreeMap::new();
    for raw in raws {
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
                    previous.is_none_or(|prev| prev == base),
                    "shape {shape_name}: conflicting extends declarations"
                );
            }
            let shape = shapes
                .entry(CompactString::from(shape_name.as_str()))
                .or_insert_with(|| StdlibShape {
                    members: BTreeMap::new(),
                });
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
    let library = StdlibLibrary {
        version,
        environment,
        globals,
        shapes,
    };
    verify_shape_references(&library);
    library
}

/// Expand the compact `[enums.<Type>]` section into member namespaces
/// of the `Enum` global: each type is an `Enum`-shaped namespace, each
/// item an `EnumItem`-shaped read-only constant. The `Enum` global and
/// the `Enums`/`Enum`/`EnumItem` shapes are hand-authored; only the
/// tree of names is generated.
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
            let constant = convert_constant(item);
            members.insert(
                constant.value,
                StdlibEntry::Constant(StdlibValue {
                    read_only: true,
                    deprecated: constant.deprecated,
                    shape: Some(CompactString::const_new("EnumItem")),
                }),
            );
        }
        let entry = StdlibEntry::Namespace(StdlibNamespace {
            members,
            shape: Some(CompactString::const_new("Enum")),
            deprecated: enum_type.deprecated.map(convert_deprecation),
        });
        assert!(
            enum_global
                .members
                .insert(CompactString::from(type_name.as_str()), entry)
                .is_none(),
            "enum type {type_name} collides with a hand-authored Enum member"
        );
    }
}

/// Copy each extending shape's base members in, own members winning.
/// Transitive chains resolve through recursion; cycles are an authoring
/// bug and panic.
fn resolve_shape_extends(
    shapes: &mut BTreeMap<CompactString, StdlibShape>,
    extends: &BTreeMap<CompactString, CompactString>,
) {
    fn collect(
        name: &str,
        shapes: &BTreeMap<CompactString, StdlibShape>,
        extends: &BTreeMap<CompactString, CompactString>,
        chain: &mut Vec<CompactString>,
    ) -> BTreeMap<CompactString, StdlibEntry> {
        assert!(
            !chain.iter().any(|visited| visited == name),
            "shape extends cycle through {name}"
        );
        chain.push(CompactString::from(name));
        let mut members = match extends.get(name) {
            Some(base) => {
                assert!(
                    shapes.contains_key(base.as_str()),
                    "shape {name} extends unknown shape {base}"
                );
                collect(base, shapes, extends, chain)
            }
            None => BTreeMap::new(),
        };
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

/// Every shape referenced by a `returns` or `shape` field must exist in
/// the library's registry - a dangling name is an authoring typo.
fn verify_shape_references(library: &StdlibLibrary) {
    fn check(library: &StdlibLibrary, entry: &StdlibEntry, path: &str) {
        let referenced = match entry {
            StdlibEntry::Function(func) => func.returns_shape.as_ref(),
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

const LUA51_TOML: &str = include_str!("../stdlib_data/lua51.toml");
const LUA52_TOML: &str = include_str!("../stdlib_data/lua52.toml");
const LUA53_TOML: &str = include_str!("../stdlib_data/lua53.toml");
const LUA54_TOML: &str = include_str!("../stdlib_data/lua54.toml");
const LUA55_TOML: &str = include_str!("../stdlib_data/lua55.toml");
const LUAU_TOML: &str = include_str!("../stdlib_data/luau.toml");
const LUAU_ROBLOX_TOML: &str = include_str!("../stdlib_data/luau_roblox.toml");
/// Generated from the Roblox API dump - see the file headers for the
/// regen command. Both splice into the Roblox library.
const ROBLOX_API_TOML: &str = include_str!("../stdlib_data/roblox_api.toml");
const ROBLOX_ENUMS_TOML: &str = include_str!("../stdlib_data/roblox_enums.toml");

static LIB_LUA51: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA51_TOML],
        LuaVersion::Lua51,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUA52: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA52_TOML],
        LuaVersion::Lua52,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUA53: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA53_TOML],
        LuaVersion::Lua53,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUA54: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA54_TOML],
        LuaVersion::Lua54,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUA55: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA55_TOML],
        LuaVersion::Lua55,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUAU: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUAU_TOML],
        LuaVersion::Luau,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUAU_ROBLOX: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUAU_ROBLOX_TOML, ROBLOX_API_TOML, ROBLOX_ENUMS_TOML],
        LuaVersion::Luau,
        StdlibEnvironment::Roblox,
    )
});

/// Reference to the parsed library for the given version and
/// environment. The environment only matters for Luau (Standalone vs
/// Roblox); numbered Lua has a single environment. Cheap: data is
/// parsed once on first access and shared thereafter.
#[must_use]
pub fn library_for(version: LuaVersion, environment: StdlibEnvironment) -> &'static StdlibLibrary {
    match version {
        LuaVersion::Lua51 => &LIB_LUA51,
        LuaVersion::Lua52 => &LIB_LUA52,
        LuaVersion::Lua53 => &LIB_LUA53,
        LuaVersion::Lua54 => &LIB_LUA54,
        LuaVersion::Lua55 => &LIB_LUA55,
        LuaVersion::Luau => match environment {
            StdlibEnvironment::Standalone => &LIB_LUAU,
            StdlibEnvironment::Roblox => &LIB_LUAU_ROBLOX,
        },
    }
}

/// Expand a deprecation `replace_template` against positional argument
/// source slices. `%1`, `%2`, ..., `%9` refer to args[0..=8]; `%%` is a
/// literal percent. Missing arguments expand to an empty string - the
/// caller decides whether that constitutes a useful fix.
#[must_use]
pub fn expand_replace_template(template: &str, args: &[&str]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                output.push('%');
                chars.next();
            }
            Some(digit) if digit.is_ascii_digit() => {
                chars.next();
                let idx = (digit as u8 - b'0') as usize;
                if idx >= 1 && idx <= args.len() {
                    output.push_str(args[idx - 1]);
                }
            }
            _ => output.push('%'),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_template_basic() {
        assert_eq!(expand_replace_template("#%1", &["t"]), "#t");
        assert_eq!(expand_replace_template("%1 ^ %2", &["a", "b"]), "a ^ b");
        assert_eq!(expand_replace_template("load(%1)", &["x"]), "load(x)");
    }

    #[test]
    fn replace_template_escape() {
        assert_eq!(expand_replace_template("100%% %1", &["x"]), "100% x");
    }

    #[test]
    fn replace_template_missing_arg_empty() {
        assert_eq!(expand_replace_template("%1 + %2", &["a"]), "a + ");
    }

    fn parse_test_library(toml_src: &str) -> StdlibLibrary {
        parse_library(&[toml_src], LuaVersion::Luau, StdlibEnvironment::Standalone)
    }

    #[test]
    fn flat_signature_parses() {
        let lib = parse_test_library(
            r#"
            [globals.f]
            kind = "function"
            min_args = 1
            max_args = 2
            params = [{ kind = "string" }, { kind = "number", required = false }]
            "#,
        );
        let StdlibEntry::Function(func) = lib.lookup_str(&["f"]).expect("f") else {
            panic!("expected function");
        };
        assert_eq!(func.signatures.len(), 1);
        assert_eq!(func.min_args(), 1);
        assert_eq!(func.max_args(), Some(2));
    }

    #[test]
    fn overloads_parse_and_match() {
        let lib = parse_test_library(
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
        let StdlibEntry::Function(func) = lib.lookup_str(&["f"]).expect("f") else {
            panic!("expected function");
        };
        assert_eq!(func.signatures.len(), 2);
        assert_eq!(func.min_args(), 0);
        assert_eq!(func.max_args(), Some(3));
        assert!(func.accepts_arg_count(0));
        assert!(!func.accepts_arg_count(1));
        assert!(func.accepts_arg_count(2));
        assert!(func.accepts_arg_count(3));
        assert!(!func.accepts_arg_count(4));
        assert_eq!(func.matching_signatures(2).count(), 1);
        assert_eq!(func.signature_index_for_active_param(1), 1);
    }

    #[test]
    fn per_constant_deprecation_parses() {
        let lib = parse_test_library(
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
        let StdlibEntry::Function(func) = lib.lookup_str(&["gc"]).expect("gc") else {
            panic!("expected function");
        };
        let StdlibArgKind::Constant(values) = &func.primary_signature().params[0].kind else {
            panic!("expected constant param");
        };
        assert_eq!(values.len(), 2);
        assert!(values[0].deprecated.is_none());
        assert_eq!(
            values[1].deprecated.as_ref().map(|d| d.message.as_str()),
            Some("gone")
        );
    }

    #[test]
    fn shapes_resolve_through_properties() {
        let lib = parse_test_library(
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
        let entry = lib
            .lookup_str(&["game", "GetService"])
            .expect("shape traversal");
        let StdlibEntry::Function(func) = entry else {
            panic!("expected function");
        };
        assert!(func.is_method);
        assert_eq!(func.returns_shape.as_deref(), Some("Instance"));
        assert!(lib.shape_member("Instance", "FindFirstChild").is_some());
    }

    #[test]
    fn per_param_deprecation_parses() {
        let lib = parse_test_library(
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
        let StdlibEntry::Function(func) = lib.lookup_str(&["f"]).expect("f") else {
            panic!("expected function");
        };
        assert!(func.primary_signature().params[1].deprecated.is_some());
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
        let lib = parse_library(&[base, extra], LuaVersion::Luau, StdlibEnvironment::Roblox);
        assert!(lib.lookup_str(&["Enum", "Material", "Grass"]).is_some());
        assert!(lib.lookup_str(&["Enum", "KeyCode", "A"]).is_some());
        assert!(lib.shape_member("Instance", "GetChildren").is_some());
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
