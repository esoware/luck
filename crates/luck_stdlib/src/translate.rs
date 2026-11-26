//! A library from a definition file, following what each declaration means
//! in Luau and nothing more.
//!
//! A `declare name: T` is a namespace of its fields when `T` is a table type,
//! a function when it's a function type, and otherwise a value of the shape
//! `T` names. A `declare function` is a function, and a `declare extern type`
//! a shape with the members it declares, its methods colon-callable, and its
//! supertype's. A type the file declares again replaces the one before it, as
//! Luau's `loadDefinitionFile` treats a redeclared global.

use std::mem;

use luck_allocator::Allocator;
use luck_ast::{
    DeclaredExternType, DeclaredFunction, Expression, ExternTypeMember, Field, FunctionAttribute,
    FunctionType, GenericTypeList, GetSpan as _, Statement, Type, TypeDeclarationValue, TypeField,
    TypeFieldAccess,
};
use luck_diagnostics::Diagnostic;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::stdlib_map::StdlibMap;
use crate::stdlib_model::{
    StdlibArgKind, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary, StdlibNamespace,
    StdlibParam, StdlibShape, StdlibSignature, StdlibValue,
};

/// How many aliases one type may go through, which only a cycle of them
/// passes.
const MAX_ALIAS_DEPTH: usize = 32;

/// `source`, a definition file, layered over `base`, in `allocator`.
///
/// # Errors
///
/// Returns the E008 of each place `source` doesn't parse.
pub(crate) fn layer<'lib>(
    allocator: &'lib Allocator,
    base: Option<&StdlibLibrary<'lib>>,
    has_roblox: bool,
    source: &str,
) -> Result<StdlibLibrary<'lib>, Vec<Diagnostic>> {
    let ast = Allocator::new();
    let parsed = luck_parser::parse_definitions(&ast, source.as_bytes());
    if !parsed.errors.is_empty() {
        return Err(parsed.errors);
    }
    let statements = &parsed.chunk.stmts;
    let mut translation = Translation {
        allocator,
        source,
        base_shapes: base.map_or(StdlibMap::EMPTY, |library| library.shapes),
        extern_types: FxHashSet::default(),
        aliases: FxHashMap::default(),
        names: FxHashMap::default(),
        globals: base
            .map(|library| library.globals.iter().map(|(name, entry)| (name, *entry)).collect())
            .unwrap_or_default(),
        shapes: base
            .map(|library| library.shapes.iter().map(|(name, shape)| (name, *shape)).collect())
            .unwrap_or_default(),
        declares_string: false,
    };
    for statement in statements {
        if let Statement::DeclaredExternType(extern_type) = statement {
            translation.extern_types.insert(extern_type.name.text.as_str());
        } else if let Statement::TypeDeclaration(declaration) = statement
            && let TypeDeclarationValue::Alias(ty) = &declaration.value
        {
            translation.aliases.insert(declaration.name.text.as_str(), ty);
        }
    }
    for statement in statements {
        translation.declare(statement);
    }
    Ok(translation.finish(has_roblox))
}

/// What a type declares, before the entry it becomes knows whether it's
/// read-only.
enum Declared<'lib> {
    Function {
        signatures: Vec<StdlibSignature<'lib>>,
        deprecated: Option<StdlibDeprecation<'lib>>,
        returns_shape: Option<&'lib str>,
    },
    Namespace {
        members: Vec<(&'lib str, StdlibEntry<'lib>)>,
        shape: Option<&'lib str>,
    },
    Value {
        shape: Option<&'lib str>,
    },
}

impl<'lib> Declared<'lib> {
    fn of(entry: &StdlibEntry<'lib>) -> Self {
        match entry {
            StdlibEntry::Function(function) => Self::Function {
                signatures: function.signatures.to_vec(),
                deprecated: function.deprecated,
                returns_shape: function.returns_shape,
            },
            StdlibEntry::Namespace(namespace) => Self::Namespace {
                members: namespace.members.iter().map(|(name, member)| (name, *member)).collect(),
                shape: namespace.shape,
            },
            StdlibEntry::Value(value) => Self::Value { shape: value.shape },
        }
    }

    /// What a value of both types has: the overloads of two functions, the
    /// members of two tables, and a shape's methods besides a table's
    /// members.
    fn intersect(self, other: Self) -> Self {
        match (self, other) {
            (
                Self::Function { mut signatures, deprecated, returns_shape },
                Self::Function {
                    signatures: more,
                    deprecated: other_deprecated,
                    returns_shape: other_shape,
                },
            ) => {
                signatures.extend(more);
                Self::Function {
                    signatures,
                    deprecated: deprecated.or(other_deprecated),
                    returns_shape: returns_shape.filter(|_| returns_shape == other_shape),
                }
            }
            (
                Self::Namespace { mut members, shape },
                Self::Namespace { members: more, shape: other_shape },
            ) => {
                members.extend(more);
                Self::Namespace { members, shape: shape.or(other_shape) }
            }
            (Self::Namespace { members, shape }, Self::Value { shape: other_shape })
            | (Self::Value { shape: other_shape }, Self::Namespace { members, shape }) => {
                Self::Namespace { members, shape: shape.or(other_shape) }
            }
            (Self::Value { shape }, Self::Value { shape: other_shape }) => {
                Self::Value { shape: shape.or(other_shape) }
            }
            (namespace @ Self::Namespace { .. }, Self::Function { .. })
            | (Self::Function { .. }, namespace @ Self::Namespace { .. }) => namespace,
            (function @ Self::Function { .. }, Self::Value { .. })
            | (Self::Value { .. }, function @ Self::Function { .. }) => function,
        }
    }
}

struct Translation<'lib, 'ast> {
    allocator: &'lib Allocator,
    source: &'ast str,
    base_shapes: StdlibMap<'lib, StdlibShape<'lib>>,
    extern_types: FxHashSet<&'ast str>,
    aliases: FxHashMap<&'ast str, &'ast Type<'ast>>,
    /// Each name copied into the library's arena, once.
    names: FxHashMap<&'ast str, &'lib str>,
    globals: FxHashMap<&'lib str, StdlibEntry<'lib>>,
    shapes: FxHashMap<&'lib str, StdlibShape<'lib>>,
    declares_string: bool,
}

impl<'lib, 'ast> Translation<'lib, 'ast> {
    fn name(&mut self, text: &'ast str) -> &'lib str {
        let allocator = self.allocator;
        self.names.entry(text).or_insert_with(|| allocator.alloc_str(text))
    }

    fn declare(&mut self, statement: &'ast Statement<'ast>) {
        let (name, entry) = if let Statement::DeclaredGlobal(global) = statement {
            let declared = self.declared(&global.ty, &[], 0);
            (global.name.text.as_str(), self.entry(declared, true))
        } else if let Statement::DeclaredFunction(function) = statement {
            (function.name.text.as_str(), self.declared_function(function, false))
        } else if let Statement::DeclaredExternType(extern_type) = statement {
            let shape = self.extern_type(extern_type);
            let name = self.name(extern_type.name.text.as_str());
            self.shapes.insert(name, shape);
            return;
        } else {
            return;
        };
        self.declares_string |= name == "string";
        let name = self.name(name);
        self.globals.insert(name, entry);
    }

    fn extern_type(&mut self, extern_type: &'ast DeclaredExternType<'ast>) -> StdlibShape<'lib> {
        let mut members: Vec<(&'lib str, StdlibEntry<'lib>)> = Vec::new();
        let mut methods: Vec<(&'ast str, StdlibEntry<'lib>)> = Vec::new();
        for member in &extern_type.members {
            match member {
                ExternTypeMember::Field(field) => members.extend(self.field(field)),
                ExternTypeMember::Method(method) => {
                    let name = method.name.text.as_str();
                    let entry = self.declared_function(method, true);
                    let overloaded = methods.iter_mut().find(|(known, _)| *known == name);
                    match (overloaded, entry) {
                        (
                            Some((_, StdlibEntry::Function(known))),
                            StdlibEntry::Function(overload),
                        ) => {
                            let signatures = [known.signatures, overload.signatures].concat();
                            let signatures = self.allocator.alloc_slice_copy(&signatures);
                            *known = self.allocator.alloc(StdlibFunction { signatures, ..**known });
                        }
                        _ => methods.push((name, entry)),
                    }
                }
            }
        }
        for (name, entry) in methods {
            members.push((self.name(name), entry));
        }
        let extends =
            extern_type.supertype.as_ref().map(|supertype| self.name(supertype.text.as_str()));
        StdlibShape { members: self.map(members), extends }
    }

    /// A table type's or an extern type's named field, which is read-only only
    /// when it's declared `read`.
    fn field(&mut self, field: &'ast TypeField<'ast>) -> Option<(&'lib str, StdlibEntry<'lib>)> {
        let TypeField::Named { access, name, value, .. } = field else {
            return None;
        };
        let declared = self.declared(value, &[], 0);
        let entry = self.entry(declared, *access == Some(TypeFieldAccess::Read));
        Some((self.name(name.text.as_str()), entry))
    }

    fn entry(&self, declared: Declared<'lib>, is_read_only: bool) -> StdlibEntry<'lib> {
        let allocator = self.allocator;
        match declared {
            Declared::Function { signatures, deprecated, returns_shape } => {
                StdlibEntry::Function(allocator.alloc(StdlibFunction {
                    signatures: allocator.alloc_slice_copy(&signatures),
                    deprecated,
                    is_read_only,
                    is_method: false,
                    returns_shape,
                }))
            }
            Declared::Namespace { members, shape } => {
                let members = self.map(members);
                StdlibEntry::Namespace(allocator.alloc(StdlibNamespace {
                    members,
                    shape,
                    is_read_only,
                }))
            }
            Declared::Value { shape } => {
                StdlibEntry::Value(allocator.alloc(StdlibValue { is_read_only, shape }))
            }
        }
    }

    /// A global's or a method's function, which is read-only.
    fn declared_function(
        &mut self,
        function: &'ast DeclaredFunction<'ast>,
        is_method: bool,
    ) -> StdlibEntry<'lib> {
        let generics = generic_names(function.generics.as_deref());
        let typed = function.params.iter().skip(usize::from(is_method));
        let mut params: Vec<StdlibParam<'lib>> = typed
            .map(|param| match &param.type_annotation {
                Some(ty) => self.param(ty, &generics),
                None => StdlibParam { kind: StdlibArgKind::Any, is_required: false },
            })
            .collect();
        if function.vararg.is_some() {
            params.push(StdlibParam { kind: StdlibArgKind::Vararg, is_required: false });
        }
        let returns_shape =
            function.return_type.as_deref().and_then(|ty| self.returned_shape(ty, &generics));
        let signature = StdlibSignature::new(self.allocator.alloc_slice_copy(&params));
        StdlibEntry::Function(self.allocator.alloc(StdlibFunction {
            signatures: self.allocator.alloc_slice_copy(&[signature]),
            deprecated: self.deprecation(&function.attributes),
            is_read_only: true,
            is_method,
            returns_shape,
        }))
    }

    fn declared(
        &mut self,
        ty: &'ast Type<'ast>,
        generics: &[&'ast str],
        depth: usize,
    ) -> Declared<'lib> {
        match ty {
            Type::Function(function) => self.function_type(function),
            Type::Table(table) => {
                let members = table.fields.iter().filter_map(|field| self.field(field)).collect();
                Declared::Namespace { members, shape: None }
            }
            Type::Typeof(typeof_type) => {
                let global = if let Expression::Name(name) = &typeof_type.expr {
                    self.globals.get(name.text.as_str())
                } else {
                    None
                };
                global.map_or(Declared::Value { shape: None }, Declared::of)
            }
            Type::Intersection(intersection) => intersection
                .types
                .iter()
                .map(|part| self.declared(part, generics, depth))
                .reduce(Declared::intersect)
                .unwrap_or(Declared::Value { shape: None }),
            Type::Parenthesized(parenthesized) => self.declared(&parenthesized.ty, generics, depth),
            Type::Name(name)
                if !generics.contains(&name.text.as_str()) && depth < MAX_ALIAS_DEPTH =>
            {
                match self.aliases.get(name.text.as_str()) {
                    Some(alias) => self.declared(alias, &[], depth + 1),
                    None => Declared::Value { shape: self.shape(ty, generics, depth) },
                }
            }
            Type::Name(_)
            | Type::Named(_)
            | Type::Optional(_)
            | Type::Union(_)
            | Type::Pack(_)
            | Type::Nil(_)
            | Type::True(_)
            | Type::False(_)
            | Type::StringLiteral(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_) => Declared::Value { shape: self.shape(ty, generics, depth) },
        }
    }

    fn function_type(&mut self, function: &'ast FunctionType<'ast>) -> Declared<'lib> {
        let generics = generic_names(function.generics.as_deref());
        let params: Vec<StdlibParam<'lib>> = function
            .params
            .iter()
            .map(|param| {
                if matches!(param.ty, Type::Variadic(_) | Type::GenericPack(_)) {
                    StdlibParam { kind: StdlibArgKind::Vararg, is_required: false }
                } else {
                    self.param(&param.ty, &generics)
                }
            })
            .collect();
        let returns_shape = self.returned_shape(&function.return_type, &generics);
        Declared::Function {
            signatures: vec![StdlibSignature::new(self.allocator.alloc_slice_copy(&params))],
            deprecated: self.deprecation(&function.attributes),
            returns_shape,
        }
    }

    /// `@deprecated`, or `@[deprecated {use = "...", reason = "..."}]`.
    fn deprecation(
        &mut self,
        attributes: &'ast [FunctionAttribute<'ast>],
    ) -> Option<StdlibDeprecation<'lib>> {
        let attribute =
            attributes.iter().find(|attribute| attribute.name.text.as_str() == "deprecated")?;
        let mut deprecation = StdlibDeprecation { r#use: None, reason: None };
        let Some(Expression::TableConstructor(table)) =
            attribute.args.as_ref().and_then(|args| args.first())
        else {
            return Some(deprecation);
        };
        for field in &table.fields {
            let Field::Named { name, value: Expression::StringLiteral(text) } = field else {
                continue;
            };
            let Some(value) = text.as_str() else { continue };
            let value = Some(self.name(value));
            match name.text.as_str() {
                "use" => deprecation.r#use = value,
                "reason" => deprecation.reason = value,
                _ => {}
            }
        }
        Some(deprecation)
    }

    /// A param of type `ty`, which is optional when `ty` takes `nil`, as
    /// Luau's `isOptional` says: it's optional, `nil`, `any`, `unknown`, or a
    /// union with one of them.
    fn param(&mut self, ty: &'ast Type<'ast>, generics: &[&'ast str]) -> StdlibParam<'lib> {
        let (kind, is_optional) = self.arg_kind(ty, generics, 0);
        StdlibParam { kind, is_required: !is_optional }
    }

    fn arg_kind(
        &mut self,
        ty: &'ast Type<'ast>,
        generics: &[&'ast str],
        depth: usize,
    ) -> (StdlibArgKind<'lib>, bool) {
        if let Some(values) = self.singletons(ty, depth) {
            return (StdlibArgKind::Constant(values), takes_nil(ty));
        }
        match ty {
            Type::Optional(optional) => (self.arg_kind(&optional.ty, generics, depth).0, true),
            Type::Parenthesized(parenthesized) => self.arg_kind(&parenthesized.ty, generics, depth),
            Type::Union(union) => {
                let mut others = union.types.iter().filter(|part| !matches!(part, Type::Nil(_)));
                match (others.next(), others.next()) {
                    (Some(only), None) => {
                        let (kind, is_optional) = self.arg_kind(only, generics, depth);
                        (kind, is_optional || takes_nil(ty))
                    }
                    _ => (StdlibArgKind::Display(self.label(ty)), takes_nil(ty)),
                }
            }
            Type::Nil(_) => (StdlibArgKind::Any, true),
            Type::True(_) | Type::False(_) => (StdlibArgKind::Bool, false),
            Type::Function(_) => (StdlibArgKind::Function, false),
            Type::Table(_) => (StdlibArgKind::Table, false),
            Type::Name(name) => match name.text.as_str() {
                "any" | "unknown" => (StdlibArgKind::Any, true),
                "boolean" => (StdlibArgKind::Bool, false),
                "number" => (StdlibArgKind::Number, false),
                "string" => (StdlibArgKind::String, false),
                generic if generics.contains(&generic) => (StdlibArgKind::Any, false),
                aliased => match self.aliases.get(aliased) {
                    Some(alias) if depth < MAX_ALIAS_DEPTH => self.arg_kind(alias, &[], depth + 1),
                    _ => (StdlibArgKind::Display(self.label(ty)), false),
                },
            },
            Type::Named(_)
            | Type::Typeof(_)
            | Type::Intersection(_)
            | Type::Pack(_)
            | Type::StringLiteral(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_) => (StdlibArgKind::Display(self.label(ty)), false),
        }
    }

    /// The strings a union of string singletons, or one singleton, allows,
    /// through the aliases it names, with `nil` among them left out.
    fn singletons(&mut self, ty: &'ast Type<'ast>, depth: usize) -> Option<&'lib [&'lib str]> {
        let mut values: Vec<&'lib str> = Vec::new();
        let mut pending = vec![(ty, depth)];
        while let Some((part, part_depth)) = pending.pop() {
            match part {
                Type::StringLiteral(literal) => {
                    let value = self.name(literal.as_str()?);
                    if !values.contains(&value) {
                        values.push(value);
                    }
                }
                Type::Union(union) => {
                    pending.extend(union.types.iter().rev().map(|inner| (inner, part_depth)));
                }
                Type::Parenthesized(parenthesized) => pending.push((&parenthesized.ty, part_depth)),
                Type::Optional(optional) => pending.push((&optional.ty, part_depth)),
                Type::Nil(_) => {}
                Type::Name(name) if part_depth < MAX_ALIAS_DEPTH => {
                    pending.push((self.aliases.get(name.text.as_str())?, part_depth + 1));
                }
                Type::Name(_)
                | Type::Named(_)
                | Type::Typeof(_)
                | Type::Table(_)
                | Type::Function(_)
                | Type::Intersection(_)
                | Type::Pack(_)
                | Type::True(_)
                | Type::False(_)
                | Type::Variadic(_)
                | Type::GenericPack(_)
                | Type::Error(_) => return None,
            }
        }
        (!values.is_empty()).then(|| &*self.allocator.alloc_slice_copy(&values))
    }

    /// `ty` as the definition writes it, on one line.
    fn label(&mut self, ty: &'ast Type<'ast>) -> &'lib str {
        let text = &self.source[ty.span().start as usize..ty.span().end as usize];
        if text.contains(['\n', '\r', '\t']) {
            let collapsed: Vec<&str> = text.split_whitespace().collect();
            return self.allocator.alloc_str(&collapsed.join(" "));
        }
        self.name(text)
    }

    /// The shape of what a function returns first.
    fn returned_shape(
        &mut self,
        ty: &'ast Type<'ast>,
        generics: &[&'ast str],
    ) -> Option<&'lib str> {
        if let Type::Pack(pack) = ty {
            return self.shape(pack.types.first()?, generics, 0);
        }
        self.shape(ty, generics, 0)
    }

    /// The shape of a value of type `ty`: the extern type, or the `string`,
    /// it names, through optionals and aliases, the one a union of it and
    /// `nil` has, or the first of an intersection's.
    fn shape(
        &mut self,
        ty: &'ast Type<'ast>,
        generics: &[&'ast str],
        depth: usize,
    ) -> Option<&'lib str> {
        let name = match ty {
            Type::Name(name) => name.text.as_str(),
            Type::Named(named) if named.prefix.is_none() => named.name.text.as_str(),
            Type::Optional(optional) => return self.shape(&optional.ty, generics, depth),
            Type::Parenthesized(parenthesized) => {
                return self.shape(&parenthesized.ty, generics, depth);
            }
            Type::Union(union) => {
                let mut others = union.types.iter().filter(|part| !matches!(part, Type::Nil(_)));
                let shape = self.shape(others.next()?, generics, depth)?;
                return others
                    .all(|part| self.shape(part, generics, depth) == Some(shape))
                    .then_some(shape);
            }
            Type::Intersection(intersection) => {
                return intersection
                    .types
                    .iter()
                    .find_map(|part| self.shape(part, generics, depth));
            }
            Type::Typeof(typeof_type) => {
                let Expression::Name(global) = &typeof_type.expr else { return None };
                return self.globals.get(global.text.as_str())?.shape();
            }
            Type::Named(_)
            | Type::Table(_)
            | Type::Function(_)
            | Type::Pack(_)
            | Type::Nil(_)
            | Type::True(_)
            | Type::False(_)
            | Type::StringLiteral(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_) => return None,
        };
        if generics.contains(&name) {
            return None;
        }
        if name == "string" {
            return Some("string");
        }
        if self.extern_types.contains(name) || self.base_shapes.contains_key(name) {
            return Some(self.name(name));
        }
        let alias = self.aliases.get(name).filter(|_| depth < MAX_ALIAS_DEPTH)?;
        self.shape(alias, &[], depth + 1)
    }

    /// The `string` shape has a method for each function of the `string`
    /// namespace that takes a string first in every signature, with that
    /// string as the receiver, since a string's metatable indexes the
    /// namespace.
    fn string_shape(&self) -> Option<StdlibShape<'lib>> {
        let Some(StdlibEntry::Namespace(namespace)) = self.globals.get("string") else {
            return None;
        };
        let allocator = self.allocator;
        let mut methods: Vec<(&'lib str, StdlibEntry<'lib>)> = Vec::new();
        for (name, entry) in &namespace.members {
            let StdlibEntry::Function(function) = entry else { continue };
            let Some(signatures) = function
                .signatures
                .iter()
                .map(|signature| match signature.params.split_first() {
                    Some((subject, rest)) if matches!(subject.kind, StdlibArgKind::String) => {
                        Some(StdlibSignature::new(rest))
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let method = StdlibFunction {
                signatures: allocator.alloc_slice_copy(&signatures),
                is_method: true,
                ..**function
            };
            methods.push((name, StdlibEntry::Function(allocator.alloc(method))));
        }
        Some(StdlibShape { members: self.map(methods), extends: None })
    }

    fn finish(mut self, has_roblox: bool) -> StdlibLibrary<'lib> {
        if self.declares_string {
            match self.string_shape() {
                Some(shape) => self.shapes.insert("string", shape),
                None => self.shapes.remove("string"),
            };
        }
        let globals = mem::take(&mut self.globals).into_iter().collect();
        let shapes = mem::take(&mut self.shapes).into_iter().collect();
        StdlibLibrary { has_roblox, globals: self.map(globals), shapes: self.map(shapes) }
    }

    /// The map of `entries`, where a later one replaces an earlier one of the
    /// same name.
    fn map<V>(&self, mut entries: Vec<(&'lib str, V)>) -> StdlibMap<'lib, V>
    where
        V: Copy,
    {
        entries.sort_by_key(|(name, _)| *name);
        let mut kept: Vec<(&'lib str, V)> = Vec::with_capacity(entries.len());
        for (name, value) in entries {
            match kept.last_mut() {
                Some(last) if last.0 == name => last.1 = value,
                _ => kept.push((name, value)),
            }
        }
        StdlibMap::new_in(self.allocator, &kept)
    }
}

fn generic_names<'ast>(generics: Option<&'ast GenericTypeList<'ast>>) -> Vec<&'ast str> {
    generics.map_or_else(Vec::new, |list| {
        list.params.iter().map(|param| param.name.text.as_str()).collect()
    })
}

/// Whether a value of type `ty` can be `nil`, as Luau's `isOptional` says.
fn takes_nil(ty: &Type<'_>) -> bool {
    match ty {
        Type::Nil(_) | Type::Optional(_) => true,
        Type::Name(name) => matches!(name.text.as_str(), "any" | "unknown"),
        Type::Parenthesized(parenthesized) => takes_nil(&parenthesized.ty),
        Type::Union(union) => union.types.iter().any(takes_nil),
        Type::Named(_)
        | Type::Typeof(_)
        | Type::Table(_)
        | Type::Function(_)
        | Type::Intersection(_)
        | Type::Pack(_)
        | Type::True(_)
        | Type::False(_)
        | Type::StringLiteral(_)
        | Type::Variadic(_)
        | Type::GenericPack(_)
        | Type::Error(_) => false,
    }
}
