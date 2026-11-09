use luck_token::{Token, TokenKind, encode_string};

use crate::expr::Expression;
use crate::punctuated::Punctuated;
use crate::synth::Synth;
use crate::types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypePack, TypeofType, UnionType, VariadicType,
};

/// A table-type field for [`Synth::ty_table`], which fills in the spans and
/// separators a real [`TypeField`] needs.
#[derive(Debug)]
pub enum TypeFieldSpec<'name> {
    Named { access: Option<TypeFieldAccess>, name: &'name str, value: Type },
    Indexer { access: Option<TypeFieldAccess>, key: Type, value: Type },
    Array(Type),
}

/// Luau `read`/`write` access modifier on a table-type field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFieldAccess {
    Read,
    Write,
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the synthesizer is one type with its constructors split by node family"
)]
impl Synth {
    #[must_use]
    pub fn ty_named(&self, name: &str) -> Type {
        Type::Name { name: self.identifier_name(name), span: self.next_span() }
    }

    #[must_use]
    pub fn ty_qualified(&self, module: &str, name: &str) -> Type {
        Type::Named(Box::new(NamedType {
            span: self.next_span(),
            prefix: Some(self.identifier(module)),
            name: self.identifier(name),
            generics: None,
        }))
    }

    #[must_use]
    pub fn ty_generic(&self, name: &str, args: Vec<Type>) -> Type {
        Type::Named(Box::new(NamedType {
            span: self.next_span(),
            prefix: None,
            name: self.identifier(name),
            generics: Some(TypeArgs { span: self.next_span(), args: Punctuated::from_items(args) }),
        }))
    }

    /// `typeof(expr)`.
    #[must_use]
    pub fn ty_typeof(&self, expr: Expression) -> Type {
        Type::Typeof(Box::new(TypeofType { span: self.next_span(), expr }))
    }

    /// Parenthesizes a union, intersection, or function type so it stays a
    /// single member of the type being built: `(A | B)?`, `(() -> A) | B`.
    fn parenthesize_if_composite(&self, inner: Type) -> Type {
        match inner {
            Type::Union(_) | Type::Intersection(_) | Type::Function(_) => {
                self.ty_parenthesized(inner)
            }
            Type::Name { .. }
            | Type::Named(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Optional(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Singleton(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_) => inner,
        }
    }

    #[must_use]
    pub fn ty_optional(&self, inner: Type) -> Type {
        Type::Optional(Box::new(OptionalType {
            span: self.next_span(),
            type_value: self.parenthesize_if_composite(inner),
        }))
    }

    #[must_use]
    pub fn ty_union(&self, types: Vec<Type>) -> Type {
        Type::Union(Box::new(UnionType {
            span: self.next_span(),
            has_leading_pipe: false,
            types: Punctuated::from_items(
                types.into_iter().map(|inner| self.parenthesize_if_composite(inner)).collect(),
            ),
        }))
    }

    #[must_use]
    pub fn ty_intersection(&self, types: Vec<Type>) -> Type {
        Type::Intersection(Box::new(IntersectionType {
            span: self.next_span(),
            has_leading_ampersand: false,
            types: Punctuated::from_items(
                types
                    .into_iter()
                    // Luau rejects a `?` suffix beside `&`, so an optional
                    // member needs parens here but not in a union.
                    .map(|inner| {
                        if matches!(inner, Type::Optional(_)) {
                            self.ty_parenthesized(inner)
                        } else {
                            self.parenthesize_if_composite(inner)
                        }
                    })
                    .collect(),
            ),
        }))
    }

    /// `(T)`.
    #[must_use]
    pub fn ty_parenthesized(&self, inner: Type) -> Type {
        Type::Parenthesized(Box::new(ParenthesizedType {
            span: self.next_span(),
            type_value: inner,
        }))
    }

    /// `{ T }` array shorthand.
    #[must_use]
    pub fn ty_table_array(&self, element: Type) -> Type {
        self.ty_table(vec![TypeFieldSpec::Array(element)])
    }

    /// `{ name: T, ... }` named-field table type.
    #[must_use]
    pub fn ty_record(&self, fields: Vec<(&str, Type)>) -> Type {
        self.ty_table(
            fields
                .into_iter()
                .map(|(name, value)| TypeFieldSpec::Named { access: None, name, value })
                .collect(),
        )
    }

    /// A full table type mixing named fields, indexers, and an array element.
    /// Named fields and indexers can take a `read`/`write` access modifier.
    /// Luau allows the `{ T }` shorthand only alone, so an array element
    /// beside other fields becomes its `[number]: T` long form.
    #[must_use]
    pub fn ty_table(&self, fields: Vec<TypeFieldSpec<'_>>) -> Type {
        let span = self.next_span();
        let can_use_array_shorthand = fields.len() == 1;
        let built_fields = fields
            .into_iter()
            .map(|field| match field {
                TypeFieldSpec::Named { access, name, value } => TypeField::Named {
                    span: self.next_span(),
                    access: access.map(|modifier| self.access_token(modifier)),
                    name: self.identifier(name),
                    value,
                },
                TypeFieldSpec::Indexer { access, key, value } => TypeField::Indexer {
                    span: self.next_span(),
                    access: access.map(|modifier| self.access_token(modifier)),
                    key,
                    value,
                },
                TypeFieldSpec::Array(value) if can_use_array_shorthand => {
                    TypeField::Array { span: self.next_span(), access: None, value }
                }
                TypeFieldSpec::Array(value) => TypeField::Indexer {
                    span: self.next_span(),
                    access: None,
                    key: self.ty_named("number"),
                    value,
                },
            })
            .collect();
        Type::Table(Box::new(TableType { span, fields: Punctuated::from_items(built_fields) }))
    }

    fn access_token(&self, access: TypeFieldAccess) -> Token {
        self.identifier(match access {
            TypeFieldAccess::Read => "read",
            TypeFieldAccess::Write => "write",
        })
    }

    /// `(T, U) -> R` with unnamed parameters. For generics or named
    /// parameters, use [`Synth::ty_function_full`].
    #[must_use]
    pub fn ty_function(&self, params: Vec<Type>, return_type: Type) -> Type {
        self.ty_function_full(None, params.into_iter().map(|ty| (None, ty)).collect(), return_type)
    }

    /// `<T...>(name: T, U) -> R`. Each parameter is `(optional name, type)`.
    /// For a pack return, pass a [`Synth::ty_pack`].
    #[must_use]
    pub fn ty_function_full(
        &self,
        generics: Option<GenericTypeList>,
        params: Vec<(Option<&str>, Type)>,
        return_type: Type,
    ) -> Type {
        let span = self.next_span();
        let typed_params: Vec<FunctionTypeParam> = params
            .into_iter()
            .map(|(param_name, type_value)| FunctionTypeParam {
                span: self.next_span(),
                name: param_name.map(|name| self.identifier(name)),
                type_value,
            })
            .collect();
        Type::Function(Box::new(FunctionType {
            span,
            generics,
            params: Punctuated::from_items(typed_params),
            return_type,
        }))
    }

    #[must_use]
    pub fn ty_singleton_string(&self, content: &str) -> Type {
        Type::Singleton(self.token(TokenKind::StringLiteral(encode_string(content.as_bytes()))))
    }

    #[must_use]
    pub fn ty_singleton_bool(&self, value: bool) -> Type {
        let kind = if value { TokenKind::True } else { TokenKind::False };
        Type::Singleton(self.token(kind))
    }

    #[must_use]
    pub fn ty_singleton_nil(&self) -> Type {
        Type::Singleton(self.token(TokenKind::Nil))
    }

    #[must_use]
    pub fn ty_pack(&self, types: Vec<Type>) -> Type {
        let span = self.next_span();
        Type::Pack(Box::new(TypePack { span, types: Punctuated::from_items(types) }))
    }

    /// `...T` variadic pack element.
    #[must_use]
    pub fn ty_variadic(&self, element: Type) -> Type {
        Type::Variadic(Box::new(VariadicType { span: self.next_span(), type_value: element }))
    }

    /// `T...` generic pack reference.
    #[must_use]
    pub fn ty_generic_pack(&self, name: &str) -> Type {
        Type::GenericPack(Box::new(GenericPackType {
            span: self.next_span(),
            name: self.identifier(name),
        }))
    }

    /// A generic parameter list at a declaration site: `<T, U...>`. Each pair
    /// is a name and whether it's a pack (`T...`). Defaults aren't supported.
    #[must_use]
    pub fn generic_type_list(&self, params: Vec<(&str, bool)>) -> GenericTypeList {
        let span = self.next_span();
        let generic_params: Vec<GenericTypeParam> = params
            .into_iter()
            .map(|(name, is_pack)| GenericTypeParam {
                span: self.next_span(),
                name: self.identifier(name),
                is_pack,
                default: None,
            })
            .collect();
        GenericTypeList { span, params: Punctuated::from_items(generic_params) }
    }
}

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;

    use luck_token::LuaVersion;

    use super::*;

    #[test]
    fn builds_full_table_type() {
        let synth = Synth::new();
        let ty = synth.ty_table(vec![
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
        assert!(matches!(&table.fields.items[0], TypeField::Named { access: Some(_), .. }));
        assert!(matches!(&table.fields.items[1], TypeField::Indexer { .. }));
    }

    #[test]
    fn builds_remaining_type_forms() {
        let synth = Synth::new();
        assert!(matches!(synth.ty_typeof(synth.name_expr("x")), Type::Typeof(_)));
        assert!(matches!(synth.ty_parenthesized(synth.ty_named("T")), Type::Parenthesized(_)));
        assert!(matches!(synth.ty_generic_pack("T"), Type::GenericPack(_)));
        assert!(matches!(
            synth.ty_singleton_bool(true),
            Type::Singleton(Token { kind: TokenKind::True, .. })
        ));
        assert!(matches!(
            synth.ty_singleton_nil(),
            Type::Singleton(Token { kind: TokenKind::Nil, .. })
        ));

        let named_param = synth.ty_function_full(
            Some(synth.generic_type_list(vec![("T", true)])),
            vec![(Some("x"), synth.ty_named("number"))],
            synth.ty_pack(vec![]),
        );
        let Type::Function(function) = &named_param else {
            panic!("expected function type");
        };
        assert!(function.generics.is_some());
        assert!(function.params.first().unwrap().name.is_some());
        assert!(matches!(function.return_type, Type::Pack(_)));
    }

    #[test]
    fn ty_table_keeps_no_array_element_beside_other_fields() {
        // Luau: `TableType ::= '{' Type '}' | '{' [PropList] '}'`, so
        // `{ T, a: U }` is a parse error.
        let built = catch_unwind(|| {
            let synth = Synth::new().with_version(LuaVersion::Luau);
            synth.ty_table(vec![
                TypeFieldSpec::Array(synth.ty_named("T")),
                TypeFieldSpec::Named { access: None, name: "a", value: synth.ty_named("U") },
            ])
        });
        // Refusing the mix and desugaring it to `[number]: T` are both correct.
        if let Ok(Type::Table(table)) = built {
            assert!(
                !table.fields.iter().any(|field| matches!(field, TypeField::Array { .. })),
                "an array element beside named fields prints `{{ T, a: U }}`, which Luau rejects"
            );
        }
    }
}
