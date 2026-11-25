use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens as _, format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Fields, GenericArgument, Generics, Ident, Item, PathArguments, Token, Type, UseTree,
    Visibility,
};

use crate::output::line_break;

/// The types of `luck_ast` that derive `Ast`, in the order of the files that
/// define them and then of their definitions.
pub(crate) struct Schema {
    pub types: Vec<TypeDef>,
    /// Where each name the definitions use comes from.
    origins: BTreeMap<String, Origin>,
}

/// The crate a name comes from. Another crate sorts first, as its `use` item
/// does.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Origin {
    External(String),
    /// `luck_ast` itself.
    Crate,
}

pub(crate) struct TypeDef {
    pub name: String,
    pub has_lifetime: bool,
    /// `#[visit(level)]`: walking it is a level of nesting.
    pub is_level: bool,
    pub shape: Shape,
}

pub(crate) enum Shape {
    Struct(Vec<FieldDef>),
    Enum(Vec<VariantDef>),
}

pub(crate) struct FieldDef {
    pub name: String,
    pub ty: FieldType,
    /// The type as written.
    pub tokens: TokenStream,
    /// `#[visit(level)]`: visiting it is a level of nesting.
    pub is_level: bool,
    /// `#[content_eq(skip)]`.
    pub skips_content_eq: bool,
}

pub(crate) struct VariantDef {
    pub name: String,
    pub payload: Payload,
}

pub(crate) enum Payload {
    Unit,
    /// One unnamed field, with its type as written.
    Single(FieldType, TokenStream),
    Named(Vec<FieldDef>),
}

/// What the generators need to know of a field's type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FieldType {
    Span,
    /// A type that derives `Ast`.
    Node(String),
    Box(Box<Self>),
    List(Box<Self>),
    OneOrMore(Box<Self>),
    Option(Box<Self>),
    /// Anything else, such as a `bool`, an operator, or a leaf's text.
    Other,
}

impl Schema {
    pub(crate) fn load(source_dir: &Path) -> io::Result<Self> {
        let mut paths = Vec::new();
        collect_sources(source_dir, &mut paths)?;
        paths.sort();
        let mut files = Vec::with_capacity(paths.len());
        for path in &paths {
            let text = fs::read_to_string(path)?;
            let file =
                syn::parse_file(&text).map_err(|e| invalid(&format!("{}: {e}", path.display())))?;
            files.push(file);
        }
        let items: Vec<&Item> = files.iter().flat_map(|file| &file.items).collect();
        let node_names: BTreeSet<String> = items
            .iter()
            .filter_map(|item| public_type(item))
            .filter(|(_, attrs)| derives_ast(attrs))
            .map(|(ident, _)| ident.to_string())
            .collect();
        let mut origins = BTreeMap::new();
        for item in &items {
            // A `pub use` re-exports this crate's own modules.
            if let Item::Use(item_use) = item
                && !is_public(&item_use.vis)
            {
                collect_origins(&item_use.tree, None, &mut origins);
            }
        }
        for (defined, _) in items.iter().filter_map(|item| public_type(item)) {
            origins.insert(defined.to_string(), Origin::Crate);
        }
        for generated in ["AstKind", "AstType", "ContentEq", "GetSpan"] {
            origins.insert(generated.to_owned(), Origin::Crate);
        }
        let mut types = Vec::new();
        for item in items {
            if let Some(type_def) = TypeDef::from_item(item, &node_names)? {
                types.push(type_def);
            }
        }
        let schema = Self { types, origins };
        schema.validate()?;
        Ok(schema)
    }

    /// The type named `name`, which every [`FieldType::Node`] names.
    pub(crate) fn get(&self, name: &str) -> Option<&TypeDef> {
        self.types.iter().find(|type_def| type_def.name == name)
    }

    pub(crate) fn structs(&self) -> impl Iterator<Item = (&TypeDef, &[FieldDef])> {
        self.types.iter().filter_map(|type_def| match &type_def.shape {
            Shape::Struct(fields) => Some((type_def, fields.as_slice())),
            Shape::Enum(_) => None,
        })
    }

    /// Whether walking a value of `ty` visits a node.
    pub(crate) fn visits(&self, ty: &FieldType) -> bool {
        match ty {
            FieldType::Node(name) => self.get(name).is_some_and(TypeDef::is_visited),
            FieldType::Box(inner)
            | FieldType::List(inner)
            | FieldType::OneOrMore(inner)
            | FieldType::Option(inner) => self.visits(inner),
            FieldType::Span | FieldType::Other => false,
        }
    }

    /// Every struct some field holds in a `Box`.
    pub(crate) fn boxed_structs(&self) -> BTreeSet<&str> {
        fn collect<'schema>(ty: &'schema FieldType, boxed: &mut BTreeSet<&'schema str>) {
            match ty {
                FieldType::Box(inner) => {
                    if let FieldType::Node(name) = &**inner {
                        boxed.insert(name);
                    }
                    collect(inner, boxed);
                }
                FieldType::List(inner) | FieldType::OneOrMore(inner) | FieldType::Option(inner) => {
                    collect(inner, boxed);
                }
                FieldType::Span | FieldType::Node(_) | FieldType::Other => {}
            }
        }
        let mut boxed = BTreeSet::new();
        for (ty, _) in self.field_types() {
            collect(ty, &mut boxed);
        }
        boxed.retain(|name| self.get(name).is_some_and(TypeDef::is_struct));
        boxed
    }

    /// Every field's and variant payload's type, and the type as written, in
    /// definition order.
    pub(crate) fn field_types(&self) -> impl Iterator<Item = (&FieldType, &TokenStream)> {
        self.types.iter().flat_map(|type_def| {
            let pairs: Vec<(&FieldType, &TokenStream)> = match &type_def.shape {
                Shape::Struct(fields) => {
                    fields.iter().map(|field| (&field.ty, &field.tokens)).collect()
                }
                Shape::Enum(variants) => variants
                    .iter()
                    .flat_map(|variant| match &variant.payload {
                        Payload::Unit => Vec::new(),
                        Payload::Single(ty, tokens) => vec![(ty, tokens)],
                        Payload::Named(fields) => {
                            fields.iter().map(|field| (&field.ty, &field.tokens)).collect()
                        }
                    })
                    .collect(),
            };
            pairs
        })
    }

    /// `use` items for `names`, grouped by crate as AGENTS.md asks: other
    /// crates first, then `crate`. `load` checks that every name a field's
    /// type holds comes from somewhere.
    pub(crate) fn imports<'name, Names>(&self, names: Names) -> TokenStream
    where
        Names: IntoIterator<Item = &'name str>,
    {
        let mut by_origin: BTreeMap<&Origin, BTreeSet<&str>> = BTreeMap::new();
        for name in names {
            if let Some(origin) = self.origins.get(name) {
                by_origin.entry(origin).or_default().insert(name);
            }
        }
        let mut tokens = TokenStream::new();
        for (origin, origin_names) in by_origin {
            let root = match origin {
                Origin::External(krate) => krate.as_str(),
                Origin::Crate => {
                    tokens.extend(line_break());
                    "crate"
                }
            };
            tokens.extend(use_item(root, &origin_names));
        }
        tokens.extend(line_break());
        tokens
    }

    /// Whether a value of `ty` has a span: every node but a fieldless enum,
    /// and an `Option` of one when it holds one.
    pub(crate) fn has_span(&self, ty: &FieldType) -> bool {
        match ty {
            FieldType::Span => true,
            FieldType::Node(name) => self.get(name).is_some_and(TypeDef::is_visited),
            FieldType::Box(inner) | FieldType::Option(inner) => self.has_span(inner),
            FieldType::List(_) | FieldType::OneOrMore(_) | FieldType::Other => false,
        }
    }

    fn validate(&self) -> io::Result<()> {
        let mut names: BTreeSet<String> = ["Allocator", "Box", "Span"].map(str::to_owned).into();
        for (_, tokens) in self.field_types() {
            type_names(tokens, &mut names);
        }
        if let Some(unknown) = names.iter().find(|name| !self.origins.contains_key(*name)) {
            return Err(invalid(&format!("no `luck_ast` file imports or defines `{unknown}`")));
        }
        for type_def in &self.types {
            match &type_def.shape {
                Shape::Struct(fields) => self.validate_spanned_fields(&type_def.name, fields)?,
                Shape::Enum(variants) => {
                    for variant in variants {
                        if let Payload::Named(fields) = &variant.payload {
                            let name = format!("{}::{}", type_def.name, variant.name);
                            self.validate_spanned_fields(&name, fields)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// A node without a `span` field spans its fields, so at least one that
    /// isn't optional has to have a span, and none may be a list.
    fn validate_spanned_fields(&self, name: &str, fields: &[FieldDef]) -> io::Result<()> {
        if fields.iter().any(FieldDef::is_own_span) {
            return Ok(());
        }
        if fields
            .iter()
            .any(|field| matches!(field.ty, FieldType::List(_) | FieldType::OneOrMore(_)))
        {
            return Err(invalid(&format!("`{name}` needs a `span` to hold a list")));
        }
        let has_required = fields
            .iter()
            .any(|field| !matches!(field.ty, FieldType::Option(_)) && self.has_span(&field.ty));
        if has_required {
            Ok(())
        } else {
            Err(invalid(&format!("`{name}` needs a `span` or a field that always has one")))
        }
    }
}

impl TypeDef {
    fn from_item(item: &Item, nodes: &BTreeSet<String>) -> io::Result<Option<Self>> {
        let (name, generics, attrs, shape) = if let Item::Struct(item_struct) = item
            && derives_ast(&item_struct.attrs)
        {
            let Fields::Named(named) = &item_struct.fields else {
                return Err(invalid(&format!("`{}` needs named fields", item_struct.ident)));
            };
            let field_defs =
                named.named.iter().map(|field| field_def(field, nodes, Site::StructField));
            let shape = Shape::Struct(field_defs.collect::<io::Result<_>>()?);
            (&item_struct.ident, &item_struct.generics, &item_struct.attrs, shape)
        } else if let Item::Enum(item_enum) = item
            && derives_ast(&item_enum.attrs)
        {
            let variants = item_enum.variants.iter().map(|variant| variant_def(variant, nodes));
            let shape = Shape::Enum(variants.collect::<io::Result<_>>()?);
            (&item_enum.ident, &item_enum.generics, &item_enum.attrs, shape)
        } else {
            return Ok(None);
        };
        let has_lifetime = check_generics(name, generics)?;
        let site = match shape {
            Shape::Struct(_) => Site::Struct,
            Shape::Enum(_) => Site::Other,
        };
        let is_level = check_helpers(attrs, site)?.is_level;
        Ok(Some(Self { name: name.to_string(), has_lifetime, is_level, shape }))
    }

    pub(crate) fn ident(&self) -> Ident {
        format_ident!("{}", self.name)
    }

    /// The type, with the tree's lifetime if it has one.
    pub(crate) fn ty(&self) -> TokenStream {
        let ident = self.ident();
        if self.has_lifetime { quote!(#ident<'ast>) } else { quote!(#ident) }
    }

    /// The type, with an elided lifetime if it has one.
    pub(crate) fn anonymous_ty(&self) -> TokenStream {
        let ident = self.ident();
        if self.has_lifetime { quote!(#ident<'_>) } else { quote!(#ident) }
    }

    pub(crate) fn snake_name(&self) -> String {
        snake_case(&self.name)
    }

    /// The name a value of this type goes by: its snake-case name, or `ty`
    /// for `type`, which is a keyword.
    pub(crate) fn binding(&self) -> Ident {
        binding_for(&self.snake_name())
    }

    pub(crate) const fn is_struct(&self) -> bool {
        matches!(self.shape, Shape::Struct(_))
    }

    /// Whether it has a `visit_*` hook: a struct, or an enum with a variant
    /// that holds something.
    pub(crate) fn is_visited(&self) -> bool {
        match &self.shape {
            Shape::Struct(_) => true,
            Shape::Enum(variants) => {
                variants.iter().any(|variant| !matches!(variant.payload, Payload::Unit))
            }
        }
    }

    pub(crate) fn visit_ident(&self) -> Ident {
        format_ident!("visit_{}", self.snake_name())
    }

    pub(crate) fn walk_ident(&self) -> Ident {
        format_ident!("walk_{}", self.snake_name())
    }
}

impl FieldDef {
    pub(crate) fn ident(&self) -> Ident {
        format_ident!("{}", self.name)
    }

    /// Whether it's the `span` its node spans.
    pub(crate) fn is_own_span(&self) -> bool {
        self.name == "span" && self.ty == FieldType::Span
    }
}

impl VariantDef {
    pub(crate) fn ident(&self) -> Ident {
        format_ident!("{}", self.name)
    }
}

impl FieldType {
    /// The node this type holds, through any `Box`, list, or `Option`.
    pub(crate) fn node(&self) -> Option<&str> {
        match self {
            Self::Node(name) => Some(name),
            Self::Box(inner) | Self::List(inner) | Self::OneOrMore(inner) | Self::Option(inner) => {
                inner.node()
            }
            Self::Span | Self::Other => None,
        }
    }
}

/// `CamelCase` to `snake_case`, with the abbreviations AGENTS.md asks for.
pub(crate) fn snake_case(name: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for c in name.chars() {
        match words.last_mut() {
            Some(word) if !c.is_ascii_uppercase() => word.push(c),
            Some(_) | None => words.push(c.to_ascii_lowercase().to_string()),
        }
    }
    let abbreviated: Vec<&str> = words
        .iter()
        .map(|word| match word.as_str() {
            "expression" => "expr",
            "statement" => "stmt",
            "parameter" => "param",
            other => other,
        })
        .collect();
    abbreviated.join("_")
}

/// The name a value goes by when its snake-case name is `snake`.
pub(crate) fn binding_for(snake: &str) -> Ident {
    if snake == "type" { format_ident!("ty") } else { format_ident!("{snake}") }
}

/// Every `.rs` file under `directory`, but those in `generated/`.
fn collect_sources(directory: &Path, paths: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let is_generated = path.file_name().is_some_and(|name| name == "generated");
        if path.is_dir() && !is_generated {
            collect_sources(&path, paths)?;
        }
        if path.extension().is_some_and(|extension| extension == "rs") {
            paths.push(path);
        }
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

const fn is_public(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public(_))
}

/// The name and attributes of a public struct or enum.
fn public_type(item: &Item) -> Option<(&Ident, &[Attribute])> {
    if let Item::Struct(item_struct) = item
        && is_public(&item_struct.vis)
    {
        return Some((&item_struct.ident, &item_struct.attrs));
    }
    if let Item::Enum(item_enum) = item
        && is_public(&item_enum.vis)
    {
        return Some((&item_enum.ident, &item_enum.attrs));
    }
    None
}

fn variant_def(variant: &syn::Variant, nodes: &BTreeSet<String>) -> io::Result<VariantDef> {
    check_helpers(&variant.attrs, Site::Other)?;
    let payload = match &variant.fields {
        Fields::Unit => Payload::Unit,
        Fields::Unnamed(unnamed) => {
            let unnamed_fields: Vec<&syn::Field> = unnamed.unnamed.iter().collect();
            let [field] = unnamed_fields.as_slice() else {
                return Err(invalid(&format!("`{}` needs one unnamed field", variant.ident)));
            };
            check_helpers(&field.attrs, Site::Other)?;
            Payload::Single(field_type(&field.ty, nodes), field.ty.to_token_stream())
        }
        Fields::Named(named) => {
            let field_defs =
                named.named.iter().map(|field| field_def(field, nodes, Site::VariantField));
            Payload::Named(field_defs.collect::<io::Result<_>>()?)
        }
    };
    Ok(VariantDef { name: variant.ident.to_string(), payload })
}

fn derives_ast(attrs: &[Attribute]) -> bool {
    attrs.iter().filter(|attr| attr.path().is_ident("derive")).any(|attr| {
        attr.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
            .is_ok_and(|paths| paths.iter().any(|path| path.is_ident("Ast")))
    })
}

/// Records where each name a `use` tree brings in from another crate comes
/// from.
fn collect_origins(tree: &UseTree, root: Option<&str>, origins: &mut BTreeMap<String, Origin>) {
    match tree {
        UseTree::Path(use_path) => {
            let segment = use_path.ident.to_string();
            collect_origins(&use_path.tree, Some(root.unwrap_or(&segment)), origins);
        }
        UseTree::Name(use_name) => {
            if let Some(krate) = root.filter(|krate| !matches!(*krate, "crate" | "super" | "self"))
            {
                origins.insert(use_name.ident.to_string(), Origin::External(krate.to_owned()));
            }
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_origins(item, root, origins);
            }
        }
        UseTree::Rename(_) | UseTree::Glob(_) => {}
    }
}

fn use_item(krate: &str, names: &BTreeSet<&str>) -> TokenStream {
    let root = Ident::new(krate, Span::call_site());
    let idents = names.iter().map(|name| format_ident!("{name}"));
    quote!(use #root::{#(#idents),*};)
}

/// Whether the type has the lifetime `'ast`.
fn check_generics(name: &Ident, generics: &Generics) -> io::Result<bool> {
    let lifetimes: Vec<String> =
        generics.lifetimes().map(|lifetime| lifetime.lifetime.ident.to_string()).collect();
    match lifetimes.as_slice() {
        [] if generics.params.is_empty() => Ok(false),
        [lifetime] if lifetime == "ast" && generics.params.len() == 1 => Ok(true),
        _ => Err(invalid(&format!("`{name}` may have only the lifetime `'ast`"))),
    }
}

/// What the tool's attributes are written on, which decides which apply.
#[derive(Clone, Copy)]
enum Site {
    Struct,
    StructField,
    VariantField,
    /// An enum, a variant, or a variant's unnamed field, which take none.
    Other,
}

impl Site {
    /// Whether `#[visit(level)]` applies.
    const fn takes_level(self) -> bool {
        match self {
            Self::Struct | Self::StructField => true,
            Self::VariantField | Self::Other => false,
        }
    }

    /// Whether `#[content_eq(skip)]` applies.
    const fn takes_skip(self) -> bool {
        match self {
            Self::StructField | Self::VariantField => true,
            Self::Struct | Self::Other => false,
        }
    }
}

struct Helpers {
    is_level: bool,
    skips_content_eq: bool,
}

/// The tool's attributes in `attrs`, written on `site`.
fn check_helpers(attrs: &[Attribute], site: Site) -> io::Result<Helpers> {
    let mut helpers = Helpers { is_level: false, skips_content_eq: false };
    for attr in attrs {
        let (expected, is_allowed, flag) = if attr.path().is_ident("visit") {
            ("level", site.takes_level(), &mut helpers.is_level)
        } else if attr.path().is_ident("content_eq") {
            ("skip", site.takes_skip(), &mut helpers.skips_content_eq)
        } else {
            continue;
        };
        let arguments = attr
            .parse_args_with(Punctuated::<Ident, Token![,]>::parse_terminated)
            .map_err(|e| invalid(&e.to_string()))?;
        let is_expected = arguments.len() == 1 && arguments.iter().all(|ident| ident == expected);
        if !is_expected || !is_allowed {
            let attribute = attr.to_token_stream();
            return Err(invalid(&format!("`{attribute}` doesn't apply here")));
        }
        *flag = true;
    }
    Ok(helpers)
}

fn field_def(field: &syn::Field, nodes: &BTreeSet<String>, site: Site) -> io::Result<FieldDef> {
    let helpers = check_helpers(&field.attrs, site)?;
    let name = field.ident.as_ref().expect("only a named field reaches here").to_string();
    Ok(FieldDef {
        name,
        ty: field_type(&field.ty, nodes),
        tokens: field.ty.to_token_stream(),
        is_level: helpers.is_level,
        skips_content_eq: helpers.skips_content_eq,
    })
}

fn field_type(ty: &Type, nodes: &BTreeSet<String>) -> FieldType {
    let Type::Path(type_path) = ty else {
        return FieldType::Other;
    };
    let Some(segment) = type_path.path.segments.last() else {
        return FieldType::Other;
    };
    let name = segment.ident.to_string();
    let inner = || {
        let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
            return FieldType::Other;
        };
        let inner_ty = arguments.args.iter().find_map(|argument| {
            if let GenericArgument::Type(argument_ty) = argument { Some(argument_ty) } else { None }
        });
        inner_ty.map_or(FieldType::Other, |argument_ty| field_type(argument_ty, nodes))
    };
    match name.as_str() {
        "Span" => FieldType::Span,
        "Box" => FieldType::Box(Box::new(inner())),
        "List" => FieldType::List(Box::new(inner())),
        "OneOrMore" => FieldType::OneOrMore(Box::new(inner())),
        "Option" => FieldType::Option(Box::new(inner())),
        _ if nodes.contains(&name) => FieldType::Node(name),
        _ => FieldType::Other,
    }
}

/// Every name a type as written refers to, but the prelude's.
pub(crate) fn type_names(tokens: &TokenStream, names: &mut BTreeSet<String>) {
    for token in tokens.clone() {
        match token {
            proc_macro2::TokenTree::Ident(ident) => {
                let name = ident.to_string();
                let is_type = name.starts_with(|c: char| c.is_ascii_uppercase());
                if is_type && !matches!(name.as_str(), "Option" | "Self") {
                    names.insert(name);
                }
            }
            proc_macro2::TokenTree::Group(group) => type_names(&group.stream(), names),
            proc_macro2::TokenTree::Punct(_) | proc_macro2::TokenTree::Literal(_) => {}
        }
    }
}
