use crate::expr::Expression;
use crate::synth::Synth;
use crate::types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypeFieldAccess, TypePack, TypeofType, UnionType, VariadicType,
};

/// A table-type field for [`Synth::ty_table`], which fills in the spans a
/// real [`TypeField`] needs.
#[derive(Debug)]
pub enum TypeFieldSpec<'name, 'ast> {
    Named { access: Option<TypeFieldAccess>, name: &'name str, value: Type<'ast> },
    Indexer { access: Option<TypeFieldAccess>, key: Type<'ast>, value: Type<'ast> },
    Array(Type<'ast>),
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the synthesizer is one type with its constructors split by node family"
)]
impl<'ast> Synth<'ast> {
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn ty_named(&self, name: &str) -> Type<'ast> {
        Type::Name(self.boxed(self.name(name)))
    }

    /// # Panics
    ///
    /// Panics if `module` or `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn ty_qualified(&self, module: &str, name: &str) -> Type<'ast> {
        Type::Named(self.boxed(NamedType {
            span: self.next_span(),
            prefix: Some(self.name(module)),
            name: self.name(name),
            generics: None,
        }))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn ty_generic<Args>(&self, name: &str, args: Args) -> Type<'ast>
    where
        Args: IntoIterator<Item = Type<'ast>>,
        Args::IntoIter: ExactSizeIterator,
    {
        Type::Named(self.boxed(NamedType {
            span: self.next_span(),
            prefix: None,
            name: self.name(name),
            generics: Some(TypeArgs { span: self.next_span(), args: self.list(args) }),
        }))
    }

    #[must_use]
    pub fn ty_typeof(&self, expr: Expression<'ast>) -> Type<'ast> {
        Type::Typeof(self.boxed(TypeofType { span: self.next_span(), expr }))
    }

    /// Parenthesizes a union, intersection, or function type so it stays a
    /// single member of the type being built: `(A | B)?`, `(() -> A) | B`.
    fn parenthesize_if_composite(&self, inner: Type<'ast>) -> Type<'ast> {
        match inner {
            Type::Union(_) | Type::Intersection(_) | Type::Function(_) => {
                self.ty_parenthesized(inner)
            }
            Type::Name(_)
            | Type::Named(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Optional(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Nil(_)
            | Type::True(_)
            | Type::False(_)
            | Type::StringLiteral(_)
            | Type::Number(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_) => inner,
        }
    }

    #[must_use]
    pub fn ty_optional(&self, inner: Type<'ast>) -> Type<'ast> {
        let ty = self.parenthesize_if_composite(inner);
        Type::Optional(self.boxed(OptionalType { span: self.next_span(), ty }))
    }

    #[must_use]
    pub fn ty_union<Types>(&self, types: Types) -> Type<'ast>
    where
        Types: IntoIterator<Item = Type<'ast>>,
        Types::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        let members =
            self.list(types.into_iter().map(|inner| self.parenthesize_if_composite(inner)));
        Type::Union(self.boxed(UnionType { span, has_leading_pipe: false, types: members }))
    }

    #[must_use]
    pub fn ty_intersection<Types>(&self, types: Types) -> Type<'ast>
    where
        Types: IntoIterator<Item = Type<'ast>>,
        Types::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        // Luau rejects a `?` suffix beside `&`, so an optional member needs
        // parens here but not in a union.
        let members = self.list(types.into_iter().map(|inner| {
            if matches!(inner, Type::Optional(_)) {
                self.ty_parenthesized(inner)
            } else {
                self.parenthesize_if_composite(inner)
            }
        }));
        Type::Intersection(self.boxed(IntersectionType {
            span,
            has_leading_ampersand: false,
            types: members,
        }))
    }

    #[must_use]
    pub fn ty_parenthesized(&self, inner: Type<'ast>) -> Type<'ast> {
        Type::Parenthesized(self.boxed(ParenthesizedType { span: self.next_span(), ty: inner }))
    }

    #[must_use]
    pub fn ty_table_array(&self, element: Type<'ast>) -> Type<'ast> {
        self.ty_table([TypeFieldSpec::Array(element)])
    }

    /// # Panics
    ///
    /// Panics if a field's name isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn ty_record<'name, Fields>(&self, fields: Fields) -> Type<'ast>
    where
        Fields: IntoIterator<Item = (&'name str, Type<'ast>)>,
        Fields::IntoIter: ExactSizeIterator,
    {
        self.ty_table(fields.into_iter().map(|(name, value)| TypeFieldSpec::Named {
            access: None,
            name,
            value,
        }))
    }

    /// Named fields and indexers can take a `read`/`write` access modifier.
    /// Luau allows the `{ T }` shorthand only alone, so an array element
    /// beside other fields becomes its `[number]: T` long form, as does one
    /// that opens with the name `read` or `write`, which the shorthand would
    /// read back as an access modifier.
    ///
    /// # Panics
    ///
    /// Panics if a [`TypeFieldSpec::Named`] name isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn ty_table<'name, Fields>(&self, fields: Fields) -> Type<'ast>
    where
        Fields: IntoIterator<Item = TypeFieldSpec<'name, 'ast>>,
        Fields::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        let specs = fields.into_iter();
        let can_use_array_shorthand = specs.len() == 1;
        let built = self.list(specs.map(|spec| match spec {
            TypeFieldSpec::Named { access, name, value } => {
                TypeField::Named { span: self.next_span(), access, name: self.name(name), value }
            }
            TypeFieldSpec::Indexer { access, key, value } => {
                TypeField::Indexer { span: self.next_span(), access, key, value }
            }
            TypeFieldSpec::Array(value)
                if can_use_array_shorthand
                    && !matches!(leading_name(&value), Some("read" | "write")) =>
            {
                TypeField::Array { span: self.next_span(), access: None, value }
            }
            TypeFieldSpec::Array(value) => TypeField::Indexer {
                span: self.next_span(),
                access: None,
                key: self.ty_named("number"),
                value,
            },
        }));
        Type::Table(self.boxed(TableType { span, fields: built, has_trailing_separator: false }))
    }

    /// For generics or named parameters, use [`Synth::ty_function_full`].
    #[must_use]
    pub fn ty_function<Params>(&self, params: Params, return_type: Type<'ast>) -> Type<'ast>
    where
        Params: IntoIterator<Item = Type<'ast>>,
        Params::IntoIter: ExactSizeIterator,
    {
        self.ty_function_full(None, params.into_iter().map(|ty| (None, ty)), return_type)
    }

    /// Each parameter is `(optional name, type)`. For a pack return, pass a [`Synth::ty_pack`].
    ///
    /// # Panics
    ///
    /// Panics if a parameter's name isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn ty_function_full<'name, Params>(
        &self,
        generics: Option<GenericTypeList<'ast>>,
        params: Params,
        return_type: Type<'ast>,
    ) -> Type<'ast>
    where
        Params: IntoIterator<Item = (Option<&'name str>, Type<'ast>)>,
        Params::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        let typed = self.list(params.into_iter().map(|(param_name, ty)| FunctionTypeParam {
            name: param_name.map(|name| self.name(name)),
            ty,
        }));
        Type::Function(self.boxed(FunctionType {
            span,
            generics: generics.map(|list| self.boxed(list)),
            params: typed,
            return_type,
        }))
    }

    #[must_use]
    pub fn ty_singleton_string(&self, content: &str) -> Type<'ast> {
        Type::StringLiteral(self.boxed(self.quoted(content.as_bytes())))
    }

    #[must_use]
    pub fn ty_singleton_bool(&self, value: bool) -> Type<'ast> {
        if value { Type::True(self.next_span()) } else { Type::False(self.next_span()) }
    }

    #[must_use]
    pub fn ty_singleton_nil(&self) -> Type<'ast> {
        Type::Nil(self.next_span())
    }

    #[must_use]
    pub fn ty_pack<Types>(&self, types: Types) -> Type<'ast>
    where
        Types: IntoIterator<Item = Type<'ast>>,
        Types::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        Type::Pack(self.boxed(TypePack { span, types: self.list(types) }))
    }

    #[must_use]
    pub fn ty_variadic(&self, element: Type<'ast>) -> Type<'ast> {
        Type::Variadic(self.boxed(VariadicType { span: self.next_span(), ty: element }))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn ty_generic_pack(&self, name: &str) -> Type<'ast> {
        Type::GenericPack(
            self.boxed(GenericPackType { span: self.next_span(), name: self.name(name) }),
        )
    }

    /// Each pair is a name and whether it's a pack (`T...`). Defaults aren't supported.
    ///
    /// # Panics
    ///
    /// Panics if a parameter's name isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn generic_type_list<'name, Params>(&self, params: Params) -> GenericTypeList<'ast>
    where
        Params: IntoIterator<Item = (&'name str, bool)>,
        Params::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        let generic_params =
            self.list(params.into_iter().map(|(name, is_pack)| GenericTypeParam {
                span: self.next_span(),
                name: self.name(name),
                is_pack,
                default: None,
            }));
        GenericTypeList { span, params: generic_params }
    }
}

/// The name `ty` prints first, if it opens with one.
fn leading_name<'ast>(ty: &Type<'ast>) -> Option<&'ast str> {
    let mut current = ty;
    loop {
        current = match current {
            Type::Name(name) => return Some(name.text.as_str()),
            Type::Named(named) => return Some(named.prefix.unwrap_or(named.name).text.as_str()),
            Type::GenericPack(pack) => return Some(pack.name.text.as_str()),
            Type::Optional(optional) => &optional.ty,
            Type::Union(union) if !union.has_leading_pipe => union.types.first()?,
            Type::Intersection(intersection) if !intersection.has_leading_ampersand => {
                intersection.types.first()?
            }
            Type::Union(_)
            | Type::Intersection(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Function(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Nil(_)
            | Type::True(_)
            | Type::False(_)
            | Type::StringLiteral(_)
            | Type::Number(_)
            | Type::Variadic(_)
            | Type::Error(_) => return None,
        };
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::Fixture;

    #[test]
    fn builds_full_table_type() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let ty = synth.ty_table([
            TypeFieldSpec::Named {
                access: Some(TypeFieldAccess::Read),
                name: "id",
                value: synth.ty_named("number"),
            },
            TypeFieldSpec::Indexer {
                access: None,
                key: synth.ty_named("string"),
                value: synth.ty_named("boolean"),
            },
        ]);
        let Type::Table(table) = &ty else {
            panic!("expected table type");
        };
        assert!(matches!(
            *table.fields,
            [
                TypeField::Named { access: Some(TypeFieldAccess::Read), .. },
                TypeField::Indexer { access: None, .. }
            ]
        ));
    }

    #[test]
    fn builds_remaining_type_forms() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert!(matches!(synth.ty_typeof(synth.name_expr("x")), Type::Typeof(_)));
        assert!(matches!(synth.ty_parenthesized(synth.ty_named("T")), Type::Parenthesized(_)));
        assert!(matches!(synth.ty_generic_pack("T"), Type::GenericPack(_)));
        assert!(matches!(synth.ty_singleton_bool(true), Type::True(_)));
        assert!(matches!(synth.ty_singleton_nil(), Type::Nil(_)));
        let Type::StringLiteral(literal) = synth.ty_singleton_string("a") else {
            panic!("expected a string singleton");
        };
        assert_eq!(literal.text, "\"a\"");

        let named_param = synth.ty_function_full(
            Some(synth.generic_type_list([("T", true)])),
            [(Some("x"), synth.ty_named("number"))],
            synth.ty_pack([]),
        );
        let Type::Function(function) = &named_param else {
            panic!("expected function type");
        };
        assert!(function.generics.is_some());
        assert!(function.params.iter().all(|param| param.name.is_some()));
        assert!(matches!(function.return_type, Type::Pack(_)));
    }

    #[test]
    fn ty_table_spells_out_an_element_the_shorthand_would_read_as_an_access_modifier() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let is_long_form = |element: Type<'_>| {
            let Type::Table(table) = synth.ty_table_array(element) else {
                panic!("expected a table type");
            };
            matches!(*table.fields, [TypeField::Indexer { .. }])
        };
        for element in [
            synth.ty_named("read"),
            synth.ty_optional(synth.ty_named("write")),
            synth.ty_union([synth.ty_named("read"), synth.ty_named("T")]),
            synth.ty_qualified("write", "T"),
        ] {
            assert!(is_long_form(element));
        }
        assert!(!is_long_form(synth.ty_named("reader")));
    }

    #[test]
    fn ty_table_keeps_no_array_element_beside_other_fields() {
        // Luau: `TableType ::= '{' Type '}' | '{' [PropList] '}'`, so
        // `{ T, a: U }` is a parse error.
        let fixture = Fixture::default();
        let synth = fixture.synth().with_version(LuaVersion::Luau);
        let Type::Table(table) = synth.ty_table([
            TypeFieldSpec::Array(synth.ty_named("T")),
            TypeFieldSpec::Named { access: None, name: "a", value: synth.ty_named("U") },
        ]) else {
            panic!("expected a table type");
        };
        assert!(
            !table.fields.iter().any(|field| matches!(field, TypeField::Array { .. })),
            "an array element beside named fields prints `{{ T, a: U }}`, which Luau rejects"
        );
    }
}
