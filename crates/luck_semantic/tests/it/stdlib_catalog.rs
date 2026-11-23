//! Generates `src/stdlib_catalog/` from `stdlib_data/`, and checks the data
//! while it does, so the catalog is static Rust and nothing about it is parsed
//! or validated at runtime.
//!
//! Each environment's TOML file is complete on its own. The Roblox library
//! also takes the generated service and enum files. Generating a library
//! splices in the enum tree, copies each extending shape's base members, and
//! derives the `string` receiver shape.
//!
//! TOML layout:
//! ```toml
//!   [globals.print]
//!   kind = "function"
//!   min_args = 0
//!   max_args = -1            # -1 = unbounded
//!   params = [{ kind = "vararg", is_required = false }]
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
//!   is_method = true
//!   ...
//! ```

use alloc::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

struct LibrarySpec {
    module: &'static str,
    sources: &'static [&'static str],
    target: &'static str,
}

const LIBRARIES: [LibrarySpec; 7] = [
    LibrarySpec { module: "lua51", sources: &["lua51.toml"], target: "Lua51" },
    LibrarySpec { module: "lua52", sources: &["lua52.toml"], target: "Lua52" },
    LibrarySpec { module: "lua53", sources: &["lua53.toml"], target: "Lua53" },
    LibrarySpec { module: "lua54", sources: &["lua54.toml"], target: "Lua54" },
    LibrarySpec { module: "lua55", sources: &["lua55.toml"], target: "Lua55" },
    LibrarySpec { module: "luau", sources: &["luau.toml"], target: "Luau" },
    LibrarySpec {
        module: "luau_roblox",
        sources: &["luau_roblox.toml", "roblox_api.toml", "roblox_enums.toml"],
        target: "LuauRoblox",
    },
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFile {
    #[serde(default)]
    globals: BTreeMap<String, Entry>,
    #[serde(default)]
    shapes: BTreeMap<String, Shape>,
    /// Named value lists that a param in any of the library's files can
    /// use with `{ kind = "constant", set = "<name>" }`, so the large
    /// generated Roblox service and class lists are written once.
    #[serde(default)]
    constant_sets: BTreeMap<String, ConstantSet>,
    /// The generated Roblox enum tree. Each `[enums.<Type>]` becomes an
    /// `Enum`-shaped namespace under the `Enum` global, with its items as
    /// read-only `EnumItem`-shaped constants. The `Enum` global itself must
    /// be hand-authored.
    #[serde(default)]
    enums: BTreeMap<String, EnumType>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConstantSet {
    values: Vec<Constant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnumType {
    items: Vec<Constant>,
    #[serde(default)]
    deprecated: Option<Deprecation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shape {
    /// Base shape whose members are copied in; own members win. Lets Roblox
    /// shapes share `Instance`'s members without modeling the class
    /// hierarchy.
    #[serde(default)]
    extends: Option<String>,
    #[serde(default)]
    members: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Entry {
    Function(Function),
    Constant(Value),
    Property(Value),
    Namespace(Namespace),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the catalog TOML, and all sixteen combinations are legal"
)]
struct RawFunction {
    #[serde(default)]
    params: Option<Vec<Param>>,
    #[serde(default)]
    min_args: Option<usize>,
    #[serde(default)]
    max_args: Option<i32>,
    #[serde(default)]
    overloads: Vec<Signature>,
    #[serde(default)]
    is_pure: bool,
    #[serde(default)]
    is_must_use: bool,
    #[serde(default)]
    deprecated: Option<Deprecation>,
    #[serde(default = "default_true")]
    is_read_only: bool,
    #[serde(default)]
    is_method: bool,
    #[serde(default)]
    returns: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawFunction")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors StdlibFunction, and all sixteen combinations are legal"
)]
struct Function {
    signatures: Vec<Signature>,
    is_pure: bool,
    is_must_use: bool,
    deprecated: Option<Deprecation>,
    is_read_only: bool,
    is_method: bool,
    returns_shape: Option<String>,
}

impl TryFrom<RawFunction> for Function {
    type Error = &'static str;

    fn try_from(raw: RawFunction) -> Result<Self, Self::Error> {
        let signatures = if raw.overloads.is_empty() {
            let (Some(min_args), Some(max_args)) = (raw.min_args, raw.max_args) else {
                return Err("a function without overloads needs min_args and max_args");
            };
            let params = raw.params.unwrap_or_default();
            vec![Signature::from(RawSignature { params, min_args, max_args })]
        } else if raw.min_args.is_some() || raw.max_args.is_some() || raw.params.is_some() {
            return Err("a function takes flat signature fields or overloads, not both");
        } else {
            raw.overloads
        };
        Ok(Self {
            signatures,
            is_pure: raw.is_pure,
            is_must_use: raw.is_must_use,
            deprecated: raw.deprecated,
            is_read_only: raw.is_read_only,
            is_method: raw.is_method,
            returns_shape: raw.returns,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSignature {
    #[serde(default)]
    params: Vec<Param>,
    min_args: usize,
    /// Negative (by convention `-1`) means unbounded, since TOML has no
    /// null.
    max_args: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(from = "RawSignature")]
struct Signature {
    params: Vec<Param>,
    min_args: usize,
    max_args: Option<usize>,
}

impl From<RawSignature> for Signature {
    fn from(raw: RawSignature) -> Self {
        Self {
            params: raw.params,
            min_args: raw.min_args,
            max_args: usize::try_from(raw.max_args).ok(),
        }
    }
}

// No `deny_unknown_fields`: serde can't combine it with the flattened kind
// tag.
#[derive(Debug, Clone, Deserialize)]
struct Param {
    #[serde(flatten)]
    kind: ArgKind,
    #[serde(default = "default_true")]
    is_required: bool,
    #[serde(default)]
    deprecated: Option<Deprecation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArgKind {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    Display {
        display: String,
    },
    /// Inline `values` or a `[constant_sets.<name>]` reference, but not both.
    Constant {
        #[serde(default)]
        values: Vec<Constant>,
        #[serde(default)]
        set: Option<String>,
    },
    Vararg,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawConstant {
    Plain(String),
    Detailed {
        value: String,
        #[serde(default)]
        deprecated: Option<Deprecation>,
        #[serde(default)]
        is_common: bool,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(from = "RawConstant")]
struct Constant {
    value: String,
    deprecated: Option<Deprecation>,
    is_common: bool,
}

impl From<RawConstant> for Constant {
    fn from(raw: RawConstant) -> Self {
        match raw {
            RawConstant::Plain(value) => Self { value, deprecated: None, is_common: false },
            RawConstant::Detailed { value, deprecated, is_common } => {
                Self { value, deprecated, is_common }
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Value {
    #[serde(default = "default_true")]
    is_read_only: bool,
    #[serde(default)]
    deprecated: Option<Deprecation>,
    #[serde(default)]
    shape: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Deprecation {
    message: String,
    #[serde(default)]
    replacement: Option<Replacement>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    template: String,
    is_exact: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Namespace {
    #[serde(default)]
    members: BTreeMap<String, Entry>,
    #[serde(default)]
    shape: Option<String>,
    #[serde(default)]
    deprecated: Option<Deprecation>,
}

const fn default_true() -> bool {
    true
}

/// A library as the catalog holds it, with every file folded in.
#[derive(Debug)]
struct Library {
    globals: BTreeMap<String, Entry>,
    shapes: BTreeMap<String, BTreeMap<String, Entry>>,
    sets: BTreeMap<String, Vec<Constant>>,
}

/// # Errors
///
/// Fails when a file isn't ASCII or doesn't parse, when two files define the
/// same name, or when the data is inconsistent.
fn compose(sources: &[(&str, String)]) -> Result<Library, String> {
    let mut library =
        Library { globals: BTreeMap::new(), shapes: BTreeMap::new(), sets: BTreeMap::new() };
    let mut extends = BTreeMap::new();
    let mut enums = BTreeMap::new();
    for (file, text) in sources {
        if let Some(offset) = text.find(|c: char| !c.is_ascii()) {
            return Err(format!("{file}: non-ASCII text at byte {offset}"));
        }
        let source: SourceFile = toml::from_str(text).map_err(|e| format!("{file}: {e}"))?;
        for (name, entry) in source.globals {
            if library.globals.insert(name.clone(), entry).is_some() {
                return Err(format!("global {name} is defined in two files"));
            }
        }
        for (name, shape) in source.shapes {
            if let Some(base) = shape.extends {
                extends.insert(name.clone(), base);
            }
            if library.shapes.insert(name.clone(), shape.members).is_some() {
                return Err(format!("shape {name} is defined in two files"));
            }
        }
        for (name, set) in source.constant_sets {
            if library.sets.insert(name.clone(), set.values).is_some() {
                return Err(format!("constant set {name} is defined in two files"));
            }
        }
        for (name, enum_type) in source.enums {
            if enums.insert(name.clone(), enum_type).is_some() {
                return Err(format!("enum type {name} is defined in two files"));
            }
        }
    }
    check_constant_params(&library)?;
    splice_enums(enums, &mut library.globals)?;
    resolve_shape_extends(&mut library.shapes, &extends)?;
    derive_string_shape(&library.globals, &mut library.shapes)?;
    Ok(library)
}

/// # Errors
///
/// Fails on a constant param with both or neither of inline values and a set
/// reference, or one that names an unknown set.
fn check_constant_params(library: &Library) -> Result<(), String> {
    /// # Errors
    ///
    /// Fails on the first bad constant param in `entry` or its members.
    fn check(library: &Library, path: &str, entry: &Entry) -> Result<(), String> {
        match entry {
            Entry::Function(function) => {
                for param in function.signatures.iter().flat_map(|signature| &signature.params) {
                    let ArgKind::Constant { values, set } = &param.kind else {
                        continue;
                    };
                    match (values.is_empty(), set) {
                        (false, None) => {}
                        (true, Some(set_name)) if library.sets.contains_key(set_name) => {}
                        (true, Some(set_name)) => {
                            return Err(format!("{path}: unknown constant set {set_name}"));
                        }
                        (false, Some(_)) | (true, None) => {
                            return Err(format!(
                                "{path}: a constant param takes inline values or a set, not both"
                            ));
                        }
                    }
                }
                Ok(())
            }
            Entry::Constant(_) | Entry::Property(_) => Ok(()),
            Entry::Namespace(namespace) => namespace
                .members
                .iter()
                .try_for_each(|(name, member)| check(library, &format!("{path}.{name}"), member)),
        }
    }
    for (name, entry) in &library.globals {
        check(library, name, entry)?;
    }
    for (shape_name, members) in &library.shapes {
        for (name, entry) in members {
            check(library, &format!("{shape_name}:{name}"), entry)?;
        }
    }
    Ok(())
}

/// Each type is an `Enum`-shaped namespace, each item an `EnumItem`-shaped
/// read-only constant. The `Enum` global and the `Enums`/`Enum`/`EnumItem`
/// shapes are hand-authored; only the tree of names is generated.
///
/// # Errors
///
/// Fails when the `Enum` global is absent or a type collides with one of
/// its hand-authored members.
fn splice_enums(
    enums: BTreeMap<String, EnumType>,
    globals: &mut BTreeMap<String, Entry>,
) -> Result<(), String> {
    if enums.is_empty() {
        return Ok(());
    }
    let Some(Entry::Namespace(enum_global)) = globals.get_mut("Enum") else {
        return Err("enum data needs a hand-authored Enum global namespace".to_owned());
    };
    for (type_name, enum_type) in enums {
        let members = enum_type
            .items
            .into_iter()
            .map(|item| {
                let value = Value {
                    is_read_only: true,
                    deprecated: item.deprecated,
                    shape: Some("EnumItem".to_owned()),
                };
                (item.value, Entry::Constant(value))
            })
            .collect();
        let entry = Entry::Namespace(Namespace {
            members,
            shape: Some("Enum".to_owned()),
            deprecated: enum_type.deprecated,
        });
        if enum_global.members.insert(type_name.clone(), entry).is_some() {
            return Err(format!("enum type {type_name} collides with a hand-authored Enum member"));
        }
    }
    Ok(())
}

/// Copies each extending shape's base members into it; own members win.
/// Chains resolve transitively.
///
/// # Errors
///
/// Fails on an `extends` cycle or an unknown base shape.
fn resolve_shape_extends(
    shapes: &mut BTreeMap<String, BTreeMap<String, Entry>>,
    extends: &BTreeMap<String, String>,
) -> Result<(), String> {
    /// # Errors
    ///
    /// Fails on an `extends` cycle or an unknown base shape.
    fn collect(
        name: &str,
        shapes: &BTreeMap<String, BTreeMap<String, Entry>>,
        extends: &BTreeMap<String, String>,
        chain: &mut Vec<String>,
    ) -> Result<BTreeMap<String, Entry>, String> {
        if chain.iter().any(|visited| visited == name) {
            return Err(format!("shape extends cycle through {name}"));
        }
        chain.push(name.to_owned());
        let mut members = match extends.get(name) {
            Some(base) if !shapes.contains_key(base) => {
                return Err(format!("shape {name} extends unknown shape {base}"));
            }
            Some(base) => collect(base, shapes, extends, chain)?,
            None => BTreeMap::new(),
        };
        if let Some(own_members) = shapes.get(name) {
            members.extend(own_members.clone());
        }
        chain.pop();
        Ok(members)
    }
    for name in extends.keys() {
        let members = collect(name, shapes, extends, &mut Vec::new())?;
        shapes.insert(name.clone(), members);
    }
    Ok(())
}

/// String namespace members whose first param isn't the subject string,
/// so they aren't methods: `string.char` takes bytes and `string.dump` a
/// function.
const NON_RECEIVER_STRING_MEMBERS: &[&str] = &["char", "dump"];

/// Members whose first return value is itself a string, so the derived
/// methods chain: `("x"):rep(2):upper()`.
const STRING_RETURNING_MEMBERS: &[&str] =
    &["format", "gsub", "lower", "pack", "rep", "reverse", "sub", "upper"];

/// Every subject-first member becomes a colon-method with the subject param
/// dropped, mirroring Lua's string metatable. Hand-authored shape members
/// win over derived ones.
///
/// # Errors
///
/// Fails when a subject-first member declares a signature with no params.
fn derive_string_shape(
    globals: &BTreeMap<String, Entry>,
    shapes: &mut BTreeMap<String, BTreeMap<String, Entry>>,
) -> Result<(), String> {
    let Some(Entry::Namespace(string_namespace)) = globals.get("string") else {
        return Ok(());
    };
    let shape = shapes.entry("string".to_owned()).or_default();
    for (name, entry) in &string_namespace.members {
        let Entry::Function(function) = entry else {
            continue;
        };
        if NON_RECEIVER_STRING_MEMBERS.contains(&name.as_str()) || shape.contains_key(name) {
            continue;
        }
        let mut method = function.clone();
        for signature in &mut method.signatures {
            if signature.params.is_empty() {
                return Err(format!("string.{name}: a receiver method needs a subject param"));
            }
            signature.params.remove(0);
            signature.min_args = signature.min_args.saturating_sub(1);
            signature.max_args = signature.max_args.map(|max| max.saturating_sub(1));
        }
        method.is_method = true;
        // Replace templates number the subject `%1`, but a colon call
        // passes it as the receiver, so keep the message and drop the fix.
        if let Some(deprecation) = &mut method.deprecated {
            deprecation.replacement = None;
        }
        if STRING_RETURNING_MEMBERS.contains(&name.as_str()) {
            method.returns_shape = Some("string".to_owned());
        }
        shape.insert(name.clone(), Entry::Function(method));
    }
    Ok(())
}

const INDENT: &str = "    ";

/// `stdlib_map!(StdlibEntry, [...])`, opened on a line at `depth`.
fn map(entries: &BTreeMap<String, Entry>, depth: usize) -> String {
    if entries.is_empty() {
        return "stdlib_map!(StdlibEntry, [])".to_owned();
    }
    let entry_depth = depth.saturating_add(1);
    let lines: Vec<String> = entries
        .iter()
        .map(|(name, value)| {
            format!("{}({name:?}, {}),\n", INDENT.repeat(entry_depth), entry(value, entry_depth))
        })
        .collect();
    format!(
        "\
stdlib_map!(StdlibEntry, [
{}{}])",
        lines.concat(),
        INDENT.repeat(depth)
    )
}

fn entry(entry: &Entry, depth: usize) -> String {
    match entry {
        Entry::Function(function) => format!(
            "StdlibEntry::Function(&{})",
            literal("StdlibFunction", &function_fields(function), "FUNCTION")
        ),
        Entry::Constant(constant)
            if constant.is_read_only
                && constant.deprecated.is_none()
                && constant.shape.as_deref() == Some("EnumItem") =>
        {
            "ENUM_ITEM".to_owned()
        }
        Entry::Constant(constant) => format!("StdlibEntry::Constant(&{})", value(constant)),
        Entry::Property(property) => format!("StdlibEntry::Property(&{})", value(property)),
        // A namespace always sets its members, so it spells out every field.
        Entry::Namespace(namespace) => {
            let shape = namespace
                .shape
                .as_ref()
                .map_or_else(|| "None".to_owned(), |shape| format!("Some({shape:?})"));
            let deprecated =
                namespace.deprecated.as_ref().map_or_else(|| "None".to_owned(), deprecation);
            format!(
                "StdlibEntry::Namespace(&StdlibNamespace {{ members: {}, shape: {shape}, deprecated: {deprecated} }})",
                map(&namespace.members, depth)
            )
        }
    }
}

/// The struct literal setting `fields` on top of the catalog's `default`,
/// or `default` itself when there are none.
fn literal(type_name: &str, fields: &[String], default: &str) -> String {
    if fields.is_empty() {
        return default.to_owned();
    }
    format!("{type_name} {{ {}, ..{default} }}", fields.join(", "))
}

fn function_fields(function: &Function) -> Vec<String> {
    let signatures: Vec<String> = function.signatures.iter().map(signature).collect();
    let mut fields = vec![format!("signatures: &[{}]", signatures.join(", "))];
    if function.is_pure {
        fields.push("is_pure: true".to_owned());
    }
    if function.is_must_use {
        fields.push("is_must_use: true".to_owned());
    }
    if let Some(deprecated) = &function.deprecated {
        fields.push(format!("deprecated: {}", deprecation(deprecated)));
    }
    if !function.is_read_only {
        fields.push("is_read_only: false".to_owned());
    }
    if function.is_method {
        fields.push("is_method: true".to_owned());
    }
    if let Some(shape) = &function.returns_shape {
        fields.push(format!("returns_shape: Some({shape:?})"));
    }
    fields
}

fn signature(signature: &Signature) -> String {
    let params: Vec<String> = signature.params.iter().map(param).collect();
    let max_args =
        signature.max_args.map_or_else(|| "None".to_owned(), |max| format!("Some({max})"));
    format!("signature(&[{}], {}, {max_args})", params.join(", "), signature.min_args)
}

fn param(param: &Param) -> String {
    let kind = arg_kind(&param.kind);
    param.deprecated.as_ref().map_or_else(
        || format!("{}({kind})", if param.is_required { "required" } else { "optional" }),
        |deprecated| {
            format!(
                "StdlibParam {{ kind: {kind}, is_required: {}, deprecated: {} }}",
                param.is_required,
                deprecation(deprecated)
            )
        },
    )
}

fn arg_kind(kind: &ArgKind) -> String {
    match kind {
        ArgKind::Any => "StdlibArgKind::Any".to_owned(),
        ArgKind::Bool => "StdlibArgKind::Bool".to_owned(),
        ArgKind::Number => "StdlibArgKind::Number".to_owned(),
        ArgKind::String => "StdlibArgKind::String".to_owned(),
        ArgKind::Function => "StdlibArgKind::Function".to_owned(),
        ArgKind::Table => "StdlibArgKind::Table".to_owned(),
        ArgKind::Display { display } => format!("StdlibArgKind::Display({display:?})"),
        ArgKind::Constant { set: Some(set_name), .. } => {
            format!("StdlibArgKind::Constant(&{})", set_name.to_ascii_uppercase())
        }
        ArgKind::Constant { values, set: None } => {
            let constants: Vec<String> = values.iter().map(constant).collect();
            format!("StdlibArgKind::Constant(&[{}])", constants.join(", "))
        }
        ArgKind::Vararg => "StdlibArgKind::Vararg".to_owned(),
    }
}

fn constant(constant: &Constant) -> String {
    let Some(deprecated) = &constant.deprecated else {
        if !constant.is_common {
            return format!("constant({:?})", constant.value);
        }
        return format!(
            "StdlibConstant {{ value: {:?}, deprecated: None, is_common: true }}",
            constant.value
        );
    };
    format!(
        "StdlibConstant {{ value: {:?}, deprecated: {}, is_common: {} }}",
        constant.value,
        deprecation(deprecated),
        constant.is_common
    )
}

fn deprecation(deprecation: &Deprecation) -> String {
    let replacement = deprecation.replacement.as_ref().map_or_else(
        || "None".to_owned(),
        |replacement| {
            format!(
                "Some(StdlibReplacement {{ template: {:?}, is_exact: {} }})",
                replacement.template, replacement.is_exact
            )
        },
    );
    format!(
        "Some(StdlibDeprecation {{ message: {:?}, replacement: {replacement} }})",
        deprecation.message
    )
}

fn value(value: &Value) -> String {
    let mut fields = Vec::new();
    if !value.is_read_only {
        fields.push("is_read_only: false".to_owned());
    }
    if let Some(deprecated) = &value.deprecated {
        fields.push(format!("deprecated: {}", deprecation(deprecated)));
    }
    if let Some(shape) = &value.shape {
        fields.push(format!("shape: Some({shape:?})"));
    }
    literal("StdlibValue", &fields, "VALUE")
}

fn shapes(shapes: &BTreeMap<String, BTreeMap<String, Entry>>) -> String {
    if shapes.is_empty() {
        return "stdlib_map!(StdlibShape, [])".to_owned();
    }
    let lines: Vec<String> = shapes
        .iter()
        .map(|(name, members)| {
            format!("{INDENT}{INDENT}({name:?}, StdlibShape {{ members: {} }}),\n", map(members, 2))
        })
        .collect();
    format!(
        "\
stdlib_map!(StdlibShape, [
{}{INDENT}])",
        lines.concat()
    )
}

fn set(name: &str, values: &[Constant]) -> String {
    let lines: Vec<String> =
        values.iter().map(|value| format!("{INDENT}{},\n", constant(value))).collect();
    format!(
        "\
static {}: [StdlibConstant; {}] = [
{}];

",
        name.to_ascii_uppercase(),
        values.len(),
        lines.concat()
    )
}

/// The catalog as the expression `stdlib_catalog` includes, which names
/// everything in that module's scope. Constant sets are statics of a block
/// around it.
fn emit(spec: &LibrarySpec, library: &Library) -> String {
    let sets: Vec<String> = library.sets.iter().map(|(name, values)| set(name, values)).collect();
    let sources: Vec<String> =
        spec.sources.iter().map(|file| format!("stdlib_data/{file}")).collect();
    let catalog = format!(
        "\
StdlibLibrary {{
{INDENT}target: LuaTarget::{},
{INDENT}globals: {},
{INDENT}shapes: {},
}}",
        spec.target,
        map(&library.globals, 1),
        shapes(&library.shapes),
    );
    let expression =
        if sets.is_empty() { catalog } else { format!("{{\n{}{catalog}\n}}", sets.concat()) };
    format!(
        "\
// Generated by `just stdlib` from {}. Never hand-edit it.

{expression}
",
        sources.join(", ")
    )
}

fn crate_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn generate(spec: &LibrarySpec) -> String {
    let sources: Vec<(&str, String)> = spec
        .sources
        .iter()
        .map(|file| {
            let text = fs::read_to_string(crate_path(&format!("stdlib_data/{file}")))
                .expect("read catalog data");
            (*file, text)
        })
        .collect();
    let library = compose(&sources).unwrap_or_else(|e| panic!("{}: {e}", spec.module));
    emit(spec, &library)
}

fn catalog_file(spec: &LibrarySpec) -> PathBuf {
    crate_path(&format!("src/stdlib_catalog/{}.rs", spec.module))
}

#[test]
#[ignore = "rewrites src/stdlib_catalog from stdlib_data; run with --ignored to refresh"]
fn regenerate_stdlib_catalog() {
    for spec in &LIBRARIES {
        fs::write(catalog_file(spec), generate(spec)).expect("write catalog");
    }
}

#[test]
fn stdlib_catalog_is_up_to_date() {
    for spec in &LIBRARIES {
        let committed = fs::read_to_string(catalog_file(spec)).expect("read catalog");
        assert!(
            committed == generate(spec),
            "src/stdlib_catalog/{}.rs is stale; regenerate it with `just stdlib`",
            spec.module
        );
    }
}

/// # Errors
///
/// Fails as [`compose`] does.
fn compose_one(toml_source: &str) -> Result<Library, String> {
    compose(&[("test.toml", toml_source.to_owned())])
}

fn function_at<'library>(library: &'library Library, path: &[&str]) -> &'library Function {
    let (root, rest) = path.split_first().expect("a path");
    let mut entry = &library.globals[*root];
    for segment in rest {
        let Entry::Namespace(namespace) = entry else {
            panic!("{segment}: not a namespace member");
        };
        entry = &namespace.members[*segment];
    }
    let Entry::Function(function) = entry else {
        panic!("{path:?}: not a function");
    };
    function
}

#[test]
fn a_flat_signature_becomes_the_only_signature() {
    let library = compose_one(
        r#"
        [globals.f]
        kind = "function"
        min_args = 1
        max_args = -1
        params = [{ kind = "string" }, { kind = "vararg", is_required = false }]
        "#,
    )
    .expect("compose");
    let signatures = &function_at(&library, &["f"]).signatures;
    assert_eq!(signatures.len(), 1);
    assert_eq!(signatures[0].min_args, 1);
    assert_eq!(signatures[0].max_args, None);
    assert_eq!(signatures[0].params.len(), 2);
}

#[test]
fn overloads_keep_their_order_and_deprecations() {
    let library = compose_one(
        r#"
        [globals.gc]
        kind = "function"
        overloads = [
          { min_args = 0, max_args = 1, params = [{ kind = "constant", is_required = false, values = [
            "collect",
            { value = "setpause", deprecated = { message = "gone" } },
          ] }] },
          { min_args = 2, max_args = 2, params = [
            { kind = "number" },
            { kind = "any", deprecated = { message = "unused", replacement = { template = "gc(%1)", is_exact = true } } },
          ] },
        ]
        "#,
    )
    .expect("compose");
    let signatures = &function_at(&library, &["gc"]).signatures;
    assert_eq!(signatures.len(), 2);
    let ArgKind::Constant { values, set: None } = &signatures[0].params[0].kind else {
        panic!("expected inline constants");
    };
    assert_eq!(values[0].value, "collect");
    assert!(values[0].deprecated.is_none());
    assert_eq!(
        values[1].deprecated.as_ref().map(|deprecation| deprecation.message.as_str()),
        Some("gone")
    );
    let deprecated = signatures[1].params[1].deprecated.as_ref().expect("param deprecation");
    let replacement = deprecated.replacement.as_ref().expect("param replacement");
    assert_eq!((replacement.template.as_str(), replacement.is_exact), ("gc(%1)", true));
}

#[test]
fn a_shape_inherits_its_base_members_and_keeps_its_own() {
    let library = compose_one(
        r#"
        [shapes.Instance.members.Name]
        kind = "property"
        [shapes.Instance.members.Clone]
        kind = "function"
        is_method = true
        min_args = 0
        max_args = 0

        [shapes.Workspace]
        extends = "Instance"
        [shapes.Workspace.members.Name]
        kind = "property"
        is_read_only = false
        "#,
    )
    .expect("compose");
    let workspace = &library.shapes["Workspace"];
    assert!(workspace.contains_key("Clone"));
    let Entry::Property(name) = &workspace["Name"] else {
        panic!("expected the own property");
    };
    assert!(!name.is_read_only, "the extending shape's own member wins");
}

#[test]
fn enums_splice_under_the_enum_global() {
    let library = compose(&[
        (
            "base.toml",
            r#"
            [globals.Enum]
            kind = "namespace"
            shape = "Enums"
            "#
            .to_owned(),
        ),
        (
            "enums.toml",
            r#"
            [enums.KeyCode]
            items = ["A", { value = "Old", deprecated = { message = "gone" } }]
            "#
            .to_owned(),
        ),
    ])
    .expect("compose");
    let Entry::Namespace(enum_global) = &library.globals["Enum"] else {
        panic!("expected the Enum namespace");
    };
    let Entry::Namespace(key_code) = &enum_global.members["KeyCode"] else {
        panic!("expected an enum type namespace");
    };
    assert_eq!(key_code.shape.as_deref(), Some("Enum"));
    let Entry::Constant(item) = &key_code.members["Old"] else {
        panic!("expected an enum item");
    };
    assert_eq!(item.shape.as_deref(), Some("EnumItem"));
    assert!(item.is_read_only && item.deprecated.is_some());
}

#[test]
fn the_string_shape_derives_methods_from_the_namespace() {
    let library = compose_one(
        r#"
        [globals.string]
        kind = "namespace"
        [globals.string.members.rep]
        kind = "function"
        min_args = 2
        max_args = 2
        params = [{ kind = "string" }, { kind = "number" }]
        deprecated = { message = "old", replacement = { template = "%1:rep(%2)", is_exact = true } }
        [globals.string.members.len]
        kind = "function"
        min_args = 1
        max_args = 1
        params = [{ kind = "string" }]
        [globals.string.members.char]
        kind = "function"
        min_args = 0
        max_args = -1
        params = [{ kind = "vararg", is_required = false }]

        [shapes.string.members.len]
        kind = "function"
        is_method = true
        min_args = 0
        max_args = 0
        is_pure = true
        "#,
    )
    .expect("compose");
    let shape = &library.shapes["string"];
    assert!(!shape.contains_key("char"), "char takes no subject string");
    let Entry::Function(rep) = &shape["rep"] else {
        panic!("expected a derived method");
    };
    assert!(rep.is_method);
    assert_eq!(rep.signatures[0].params.len(), 1);
    assert_eq!((rep.signatures[0].min_args, rep.signatures[0].max_args), (1, Some(1)));
    assert_eq!(rep.returns_shape.as_deref(), Some("string"));
    let deprecation = rep.deprecated.as_ref().expect("the deprecation stays");
    assert!(deprecation.replacement.is_none(), "a template numbers the subject");
    let Entry::Function(len) = &shape["len"] else {
        panic!("expected the hand-authored method");
    };
    assert!(len.is_pure, "a hand-authored member wins over the derived one");
}

const INCONSISTENT_DATA_CASES: [(&[&str], &str); 13] = [
    (
        &["\
[globals.f]
kind = \"function\"
min_args = 0
max_args = 0
overloads = [{ min_args = 1, max_args = 1 }]"],
        "flat signature fields or overloads",
    ),
    (
        &["\
[globals.f]
kind = \"function\"
min_args = 0"],
        "needs min_args and max_args",
    ),
    (
        &["\
[globals.f]
kind = \"function\"
min_args = 1
max_args = 1
params = [{ kind = \"constant\" }]"],
        "f: a constant param takes inline values or a set",
    ),
    (
        &["\
[constant_sets.s]
values = [\"a\"]
[globals.f]
kind = \"function\"
min_args = 1
max_args = 1
params = [{ kind = \"constant\", values = [\"b\"], set = \"s\" }]"],
        "f: a constant param takes inline values or a set",
    ),
    (
        &["\
[globals.n]
kind = \"namespace\"
[globals.n.members.f]
kind = \"function\"
min_args = 1
max_args = 1
params = [{ kind = \"constant\", set = \"missing\" }]"],
        "n.f: unknown constant set missing",
    ),
    (
        &["\
[shapes.a]
extends = \"b\""],
        "shape a extends unknown shape b",
    ),
    (
        &["\
[shapes.a]
extends = \"b\"
[shapes.b]
extends = \"a\""],
        "shape extends cycle through a",
    ),
    (
        &["\
[enums.KeyCode]
items = [\"A\"]"],
        "needs a hand-authored Enum global",
    ),
    (
        &[
            "\
[globals.Enum]
kind = \"namespace\"
[globals.Enum.members.KeyCode]
kind = \"namespace\"",
            "\
[enums.KeyCode]
items = [\"A\"]",
        ],
        "enum type KeyCode collides",
    ),
    (
        &["\
[globals.string]
kind = \"namespace\"
[globals.string.members.f]
kind = \"function\"
min_args = 0
max_args = 0"],
        "string.f: a receiver method needs a subject param",
    ),
    (
        &[
            "\
[globals.g]
kind = \"property\"",
            "\
[globals.g]
kind = \"constant\"",
        ],
        "global g is defined in two files",
    ),
    (
        &[
            "\
[constant_sets.s]
values = [\"a\"]",
            "\
[constant_sets.s]
values = [\"b\"]",
        ],
        "constant set s is defined in two files",
    ),
    (&["# caf\u{e9}"], "test.toml: non-ASCII text at byte 5"),
];

#[test]
fn rejects_inconsistent_data() {
    for (files, expected) in INCONSISTENT_DATA_CASES {
        let sources: Vec<(&str, String)> =
            files.iter().map(|text| ("test.toml", (*text).to_owned())).collect();
        let error = compose(&sources).expect_err(expected);
        assert!(error.contains(expected), "expected `{expected}` in: {error}");
    }
}
