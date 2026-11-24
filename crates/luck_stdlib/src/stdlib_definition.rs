//! A std definition is a TOML file, and each file of `stdlib_data/` is one
//! for a built-in library. The README describes the format.
//!
//! What a file holds on its own is checked when it's parsed, and what it
//! names in the library below it, a shape or a constant set, when it's
//! layered.
//!
//! An entry is read as a plain table and then checked against its `kind`,
//! since serde reads an internally tagged enum through a buffer that loses
//! where in the file each value was.

use std::collections::BTreeMap;

use luck_diagnostics::Diagnostic;
use luck_token::Span;
use serde::Deserialize;
use toml::Spanned;

/// One std definition file, parsed and checked on its own, which
/// [`Stdlib::layer`](crate::Stdlib::layer) layers over a library.
///
/// `[enums.<Type>]` tables are members of its `Enum` global by now, and the
/// `string` shape has a method for each function of the `string` namespace
/// that takes a string first.
#[derive(Debug, Clone)]
pub struct StdlibDefinition {
    globals: BTreeMap<String, Entry>,
    shapes: BTreeMap<String, Shape>,
    constant_sets: BTreeMap<String, Vec<Constant>>,
}

impl StdlibDefinition {
    /// # Errors
    ///
    /// Returns E014, at the place the problem is when it has one, when
    /// `text` isn't a valid std definition.
    pub fn parse(text: &str) -> Result<Self, Diagnostic> {
        let raw: RawDefinition = toml::from_str(text).map_err(|e| {
            let span = e.span().map(|range| Span::from_indices(range.start, range.end));
            Diagnostic::e014(span, e.message())
        })?;
        let RawDefinition { mut globals, mut shapes, constant_sets: sets, enums } = raw;
        splice_enums(enums, &mut globals)?;
        derive_string_methods(&globals, &mut shapes);
        let constant_sets = sets.into_iter().map(|(name, set)| (name, set.values.0)).collect();
        Ok(Self { globals, shapes, constant_sets })
    }

    pub(crate) const fn globals(&self) -> &BTreeMap<String, Entry> {
        &self.globals
    }

    pub(crate) const fn shapes(&self) -> &BTreeMap<String, Shape> {
        &self.shapes
    }

    pub(crate) const fn constant_sets(&self) -> &BTreeMap<String, Vec<Constant>> {
        &self.constant_sets
    }
}

/// The name of a shape or a constant set, where the definition gives it.
pub(crate) type Reference = Spanned<String>;

/// Where `reference` is in its file.
pub(crate) fn place(reference: &Reference) -> Span {
    let range = reference.span();
    Span::from_indices(range.start, range.end)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDefinition {
    #[serde(default)]
    globals: BTreeMap<String, Entry>,
    #[serde(default)]
    shapes: BTreeMap<String, Shape>,
    #[serde(default)]
    constant_sets: BTreeMap<String, ConstantSet>,
    #[serde(default)]
    enums: BTreeMap<String, Spanned<EnumType>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConstantSet {
    values: NonEmpty<Constant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnumType {
    items: Vec<Constant>,
    deprecated: Option<Deprecation>,
}

/// A list the format needs at least one item in.
#[derive(Deserialize)]
#[serde(try_from = "Vec<T>")]
struct NonEmpty<T>(Vec<T>);

impl<T> TryFrom<Vec<T>> for NonEmpty<T> {
    type Error = &'static str;

    fn try_from(items: Vec<T>) -> Result<Self, Self::Error> {
        if items.is_empty() {
            return Err("this list needs at least one item");
        }
        Ok(Self(items))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntryKind {
    Function,
    Constant,
    Property,
    Namespace,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    kind: EntryKind,
    params: Option<Vec<Param>>,
    overloads: Option<NonEmpty<Overload>>,
    is_pure: Option<bool>,
    is_must_use: Option<bool>,
    is_method: Option<bool>,
    returns: Option<Reference>,
    is_read_only: Option<bool>,
    shape: Option<Reference>,
    members: Option<BTreeMap<String, Entry>>,
    deprecated: Option<Deprecation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Overload {
    #[serde(default)]
    params: Vec<Param>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawEntry")]
pub(crate) enum Entry {
    Function(Function),
    Constant(Value),
    Property(Value),
    Namespace(Namespace),
}

impl TryFrom<RawEntry> for Entry {
    type Error = String;

    fn try_from(raw: RawEntry) -> Result<Self, Self::Error> {
        let RawEntry {
            kind,
            params: flat,
            overloads: forms,
            is_pure,
            is_must_use,
            is_method,
            returns,
            is_read_only,
            shape,
            members,
            deprecated,
        } = raw;
        let function_key = [
            ("params", flat.is_some()),
            ("overloads", forms.is_some()),
            ("is_pure", is_pure.is_some()),
            ("is_must_use", is_must_use.is_some()),
            ("is_method", is_method.is_some()),
            ("returns", returns.is_some()),
        ]
        .into_iter()
        .find_map(|(key, is_set)| is_set.then_some(key));
        if let Some(key) = function_key
            && !matches!(kind, EntryKind::Function)
        {
            return Err(format!("only a `function` takes `{key}`"));
        }
        if members.is_some() && !matches!(kind, EntryKind::Namespace) {
            return Err("only a `namespace` takes `members`".to_owned());
        }
        Ok(match kind {
            EntryKind::Function => {
                if shape.is_some() {
                    return Err(
                        "a `function` takes no `shape`, but its call's `returns`".to_owned()
                    );
                }
                Self::Function(Function {
                    signatures: signatures(flat, forms)?,
                    is_pure: is_pure.unwrap_or(false),
                    is_must_use: is_must_use.unwrap_or(false),
                    deprecated,
                    is_read_only: is_read_only.unwrap_or(true),
                    is_method: is_method.unwrap_or(false),
                    returns_shape: returns,
                })
            }
            EntryKind::Constant => Self::Constant(Value {
                is_read_only: is_read_only.unwrap_or(true),
                deprecated,
                shape,
            }),
            EntryKind::Property => Self::Property(Value {
                is_read_only: is_read_only.unwrap_or(true),
                deprecated,
                shape,
            }),
            EntryKind::Namespace => {
                if is_read_only.is_some() {
                    return Err("a `namespace` is always read-only".to_owned());
                }
                Self::Namespace(Namespace {
                    members: members.unwrap_or_default(),
                    shape,
                    deprecated,
                })
            }
        })
    }
}

/// A function's signatures, from its flat `params` or its `overloads`.
fn signatures(
    flat: Option<Vec<Param>>,
    forms: Option<NonEmpty<Overload>>,
) -> Result<Vec<Vec<Param>>, String> {
    let signatures: Vec<Vec<Param>> = match (flat, forms) {
        (Some(_), Some(_)) => {
            return Err("a function takes `params` or `overloads`, not both".to_owned());
        }
        (params, None) => vec![params.unwrap_or_default()],
        (None, Some(NonEmpty(overloads))) => {
            overloads.into_iter().map(|overload| overload.params).collect()
        }
    };
    for signature in &signatures {
        let Some((_, before_last)) = signature.split_last() else {
            continue;
        };
        if before_last.iter().any(|param| matches!(param.kind, ArgKind::Vararg)) {
            return Err("only the last param can be a `vararg`".to_owned());
        }
        let fixed = signature.iter().filter(|param| !matches!(param.kind, ArgKind::Vararg));
        if !fixed.is_sorted_by_key(|param| !param.is_required) {
            return Err("a required param can't follow an optional one".to_owned());
        }
    }
    Ok(signatures)
}

#[derive(Debug, Clone)]
pub(crate) struct Function {
    /// Each signature's params, required ones first, then optional ones,
    /// then at most one vararg.
    pub signatures: Vec<Vec<Param>>,
    pub is_pure: bool,
    pub is_must_use: bool,
    pub deprecated: Option<Deprecation>,
    pub is_read_only: bool,
    pub is_method: bool,
    pub returns_shape: Option<Reference>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParam {
    kind: ParamKind,
    is_required: Option<bool>,
    deprecated: Option<Deprecation>,
    display: Option<String>,
    values: Option<NonEmpty<Constant>>,
    set: Option<Reference>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ParamKind {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    Display,
    Constant,
    Vararg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawParam")]
pub(crate) struct Param {
    pub kind: ArgKind,
    pub is_required: bool,
    pub deprecated: Option<Deprecation>,
}

impl TryFrom<RawParam> for Param {
    type Error = &'static str;

    fn try_from(raw: RawParam) -> Result<Self, Self::Error> {
        let RawParam { kind: named, is_required, deprecated, mut display, mut values, mut set } =
            raw;
        let kind = match named {
            ParamKind::Any => ArgKind::Any,
            ParamKind::Bool => ArgKind::Bool,
            ParamKind::Number => ArgKind::Number,
            ParamKind::String => ArgKind::String,
            ParamKind::Function => ArgKind::Function,
            ParamKind::Table => ArgKind::Table,
            ParamKind::Display => ArgKind::Display(
                display.take().ok_or("a `display` param needs its `display` label")?,
            ),
            ParamKind::Constant => ArgKind::Constant(match (values.take(), set.take()) {
                (Some(NonEmpty(listed)), None) => Constants::Values(listed),
                (None, Some(set_name)) => Constants::Set(set_name),
                (Some(_), Some(_)) | (None, None) => {
                    return Err("a `constant` param takes its `values` or a `set`, one of the two");
                }
            }),
            ParamKind::Vararg => ArgKind::Vararg,
        };
        if display.is_some() {
            return Err("only a `display` param takes a `display` label");
        }
        if values.is_some() || set.is_some() {
            return Err("only a `constant` param takes `values` or a `set`");
        }
        let is_vararg = matches!(kind, ArgKind::Vararg);
        if is_vararg && is_required == Some(true) {
            return Err("a `vararg` param is never required");
        }
        Ok(Self { kind, is_required: !is_vararg && is_required.unwrap_or(true), deprecated })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ArgKind {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    Display(String),
    Constant(Constants),
    Vararg,
}

#[derive(Debug, Clone)]
pub(crate) enum Constants {
    Values(Vec<Constant>),
    /// A constant set of this definition or of the library below it.
    Set(Reference),
}

#[derive(Deserialize)]
#[serde(untagged, expecting = "a constant, as its value or as a table with its `value`")]
enum RawConstant {
    Plain(String),
    Detailed(DetailedConstant),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetailedConstant {
    value: String,
    deprecated: Option<Deprecation>,
    #[serde(default)]
    is_common: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(from = "RawConstant")]
pub(crate) struct Constant {
    pub value: String,
    pub deprecated: Option<Deprecation>,
    pub is_common: bool,
}

impl From<RawConstant> for Constant {
    fn from(raw: RawConstant) -> Self {
        match raw {
            RawConstant::Plain(value) => Self { value, deprecated: None, is_common: false },
            RawConstant::Detailed(DetailedConstant { value, deprecated, is_common }) => {
                Self { value, deprecated, is_common }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Value {
    pub is_read_only: bool,
    pub deprecated: Option<Deprecation>,
    pub shape: Option<Reference>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Deprecation {
    pub message: String,
    pub replacement: Option<Replacement>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Replacement {
    pub template: String,
    pub is_exact: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Namespace {
    pub members: BTreeMap<String, Entry>,
    pub shape: Option<Reference>,
    pub deprecated: Option<Deprecation>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Shape {
    pub extends: Option<Reference>,
    #[serde(default)]
    pub members: BTreeMap<String, Entry>,
}

/// Each type is an `Enum`-shaped namespace of the `Enum` global, which a
/// definition without one gets, and each item an `EnumItem`-shaped read-only
/// constant.
fn splice_enums(
    enums: BTreeMap<String, Spanned<EnumType>>,
    globals: &mut BTreeMap<String, Entry>,
) -> Result<(), Diagnostic> {
    if enums.is_empty() {
        return Ok(());
    }
    let Entry::Namespace(enum_global) =
        globals.entry("Enum".to_owned()).or_insert_with(|| Entry::Namespace(Namespace::default()))
    else {
        return Err(Diagnostic::e014(None, "`enums` need the `Enum` global to be a namespace"));
    };
    for (type_name, spanned) in enums {
        let range = spanned.span();
        if enum_global.members.contains_key(&type_name) {
            return Err(Diagnostic::e014(
                Some(Span::from_indices(range.start, range.end)),
                &format!("the enum type `{type_name}` is also a member of the `Enum` global"),
            ));
        }
        let shape_named = |name: &str| Some(Spanned::new(range.clone(), name.to_owned()));
        let enum_type = spanned.into_inner();
        let members = enum_type
            .items
            .into_iter()
            .map(|item| {
                let value = Value {
                    is_read_only: true,
                    deprecated: item.deprecated,
                    shape: shape_named("EnumItem"),
                };
                (item.value, Entry::Constant(value))
            })
            .collect();
        let namespace =
            Namespace { members, shape: shape_named("Enum"), deprecated: enum_type.deprecated };
        enum_global.members.insert(type_name, Entry::Namespace(namespace));
    }
    Ok(())
}

/// A string's metatable indexes the `string` namespace.
///
/// So each of its functions that takes a string first in every signature is
/// also a method of the `string` shape, with that string as the receiver. A
/// member the shape declares itself wins. A method keeps a deprecation's
/// message but not its fix, since a template numbers the receiver `%1`.
fn derive_string_methods(globals: &BTreeMap<String, Entry>, shapes: &mut BTreeMap<String, Shape>) {
    let Some(Entry::Namespace(string_namespace)) = globals.get("string") else {
        return;
    };
    let shape = shapes.entry("string".to_owned()).or_default();
    for (name, entry) in &string_namespace.members {
        let Entry::Function(function) = entry else {
            continue;
        };
        let Some(signatures) = function
            .signatures
            .iter()
            .map(|params| {
                params
                    .split_first()
                    .filter(|(subject, _)| matches!(subject.kind, ArgKind::String))
                    .map(|(_, rest)| rest.to_vec())
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let deprecated = function.deprecated.as_ref().map(|deprecation| Deprecation {
            message: deprecation.message.clone(),
            replacement: None,
        });
        let method = Function { signatures, deprecated, is_method: true, ..function.clone() };
        shape.members.entry(name.clone()).or_insert(Entry::Function(method));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_error(text: &str) -> Diagnostic {
        StdlibDefinition::parse(text).expect_err("the definition is invalid")
    }

    fn placed<'text>(diag: &Diagnostic, text: &'text str) -> Option<&'text str> {
        diag.span.map(|span| str::from_utf8(span.text(text.as_bytes())).unwrap())
    }

    #[test]
    fn a_function_without_params_or_overloads_takes_nothing() {
        let definition = StdlibDefinition::parse(
            r#"
[globals.f]
kind = "function"
"#,
        )
        .expect("parses");
        let Entry::Function(function) = &definition.globals["f"] else {
            panic!("expected a function");
        };
        assert_eq!(function.signatures.len(), 1);
        assert!(function.signatures[0].is_empty());
    }

    #[test]
    fn a_vararg_is_optional_unless_it_says_otherwise() {
        let definition = StdlibDefinition::parse(
            r#"
[globals.f]
kind = "function"
params = [{ kind = "string" }, { kind = "vararg" }]
"#,
        )
        .expect("parses");
        let Entry::Function(function) = &definition.globals["f"] else {
            panic!("expected a function");
        };
        let required: Vec<bool> =
            function.signatures[0].iter().map(|param| param.is_required).collect();
        assert_eq!(required, [true, false]);
    }

    #[test]
    fn a_problem_deep_in_a_namespace_is_placed_on_it() {
        let text = r#"
[globals.love]
kind = "namespace"

[globals.love.members.graphics]
kind = "namespace"

[globals.love.members.graphics.members.print]
kind = "function"
params = [{ kind = "string" }, { kind = "numbr" }]
"#;
        let diag = parse_error(text);
        assert_eq!(diag.code, "E014");
        assert!(diag.message.starts_with("invalid config: unknown variant `numbr`"), "{diag:?}");
        assert_eq!(placed(&diag, text), Some(r#""numbr""#), "{diag:?}");
    }

    #[test]
    fn a_problem_with_an_entry_is_placed_on_its_table() {
        let text = r#"
[globals.f]
kind = "property"
is_pure = true
"#;
        let diag = parse_error(text);
        assert_eq!(diag.message, "invalid config: only a `function` takes `is_pure`");
        assert_eq!(placed(&diag, text), Some("[globals.f]"), "{diag:?}");
    }

    const INVALID_CASES: [(&str, &str); 16] = [
        (
            r#"
[globals.f]
kind = "function"
params = []
overloads = [{ params = [] }]"#,
            "a function takes `params` or `overloads`, not both",
        ),
        (
            r#"
[globals.f]
kind = "function"
overloads = []"#,
            "this list needs at least one item",
        ),
        (
            r#"
[globals.f]
kind = "function"
params = [{ kind = "vararg" }, { kind = "any" }]"#,
            "only the last param can be a `vararg`",
        ),
        (
            r#"
[globals.f]
kind = "function"
params = [{ kind = "any", is_required = false }, { kind = "any" }]"#,
            "a required param can't follow an optional one",
        ),
        (
            r#"
[globals.f]
kind = "function"
params = [{ kind = "vararg", is_required = true }]"#,
            "a `vararg` param is never required",
        ),
        (
            r#"
[globals.f]
kind = "function"
params = [{ kind = "display" }]"#,
            "a `display` param needs its `display` label",
        ),
        (
            r#"
[globals.f]
kind = "function"
params = [{ kind = "any", display = "x" }]"#,
            "only a `display` param takes a `display` label",
        ),
        (
            r#"
[globals.f]
kind = "function"
params = [{ kind = "constant" }]"#,
            "a `constant` param takes its `values` or a `set`, one of the two",
        ),
        (
            r#"
[globals.f]
kind = "function"
params = [{ kind = "string", set = "s" }]"#,
            "only a `constant` param takes `values` or a `set`",
        ),
        (
            r#"
[globals.f]
kind = "function"
shape = "file""#,
            "a `function` takes no `shape`, but its call's `returns`",
        ),
        (
            r#"
[globals.f]
kind = "constant"
members = {}"#,
            "only a `namespace` takes `members`",
        ),
        (
            r#"
[globals.n]
kind = "namespace"
is_read_only = false"#,
            "a `namespace` is always read-only",
        ),
        (
            r#"
[globals.f]
kind = "funtion""#,
            "unknown variant `funtion`",
        ),
        (
            "
[constant_sets.modes]
values = []",
            "this list needs at least one item",
        ),
        (
            r#"
[globals.Enum]
kind = "property"

[enums.KeyCode]
items = ["A"]"#,
            "`enums` need the `Enum` global to be a namespace",
        ),
        (
            r#"
[globals.Enum]
kind = "namespace"

[globals.Enum.members.KeyCode]
kind = "namespace"

[enums.KeyCode]
items = ["A"]"#,
            "the enum type `KeyCode` is also a member of the `Enum` global",
        ),
    ];

    #[test]
    fn rejects_what_the_format_rules_out() {
        for (text, expected) in INVALID_CASES {
            let diag = parse_error(text);
            assert!(diag.message.contains(expected), "expected `{expected}` in {diag:?}");
        }
    }

    #[test]
    fn enums_become_members_of_an_enum_global_the_definition_lacks() {
        let definition = StdlibDefinition::parse(
            r#"
[enums.KeyCode]
items = ["A", { value = "Old", deprecated = { message = "gone" } }]
"#,
        )
        .expect("parses");
        let Entry::Namespace(enum_global) = &definition.globals["Enum"] else {
            panic!("expected the Enum namespace");
        };
        assert!(enum_global.shape.is_none(), "the library below gives it its shape");
        let Entry::Namespace(key_code) = &enum_global.members["KeyCode"] else {
            panic!("expected an enum type namespace");
        };
        assert_eq!(key_code.shape.as_ref().map(|shape| shape.get_ref().as_str()), Some("Enum"));
        let Entry::Constant(item) = &key_code.members["Old"] else {
            panic!("expected an enum item");
        };
        assert_eq!(item.shape.as_ref().map(|shape| shape.get_ref().as_str()), Some("EnumItem"));
        assert!(item.is_read_only && item.deprecated.is_some());
    }

    #[test]
    fn the_string_shape_has_a_method_for_each_function_that_takes_a_string_first() {
        let definition = StdlibDefinition::parse(
            r#"
[globals.string]
kind = "namespace"

[globals.string.members.rep]
kind = "function"
params = [{ kind = "string" }, { kind = "number" }]
returns = "string"
deprecated = { message = "old", replacement = { template = "%1:rep(%2)", is_exact = true } }

[globals.string.members.len]
kind = "function"
params = [{ kind = "string" }]

[globals.string.members.char]
kind = "function"
params = [{ kind = "vararg" }]

[shapes.string.members.len]
kind = "function"
is_method = true
is_pure = true
"#,
        )
        .expect("parses");
        let shape = &definition.shapes["string"];
        assert!(!shape.members.contains_key("char"), "char takes no string first");
        let Entry::Function(rep) = &shape.members["rep"] else {
            panic!("expected a method");
        };
        assert!(rep.is_method);
        assert_eq!(rep.signatures[0].len(), 1);
        assert_eq!(
            rep.returns_shape.as_ref().map(|returned| returned.get_ref().as_str()),
            Some("string")
        );
        let deprecation = rep.deprecated.as_ref().expect("the deprecation stays");
        assert!(deprecation.replacement.is_none(), "a template numbers the receiver");
        let Entry::Function(len) = &shape.members["len"] else {
            panic!("expected the declared method");
        };
        assert!(len.is_pure, "the shape's own member wins");
    }
}
