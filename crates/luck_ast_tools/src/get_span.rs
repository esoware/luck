//! `GetSpan` for every node: its `span` field, or, for a node without one,
//! the span from its first field to its last. Ported from oxc's
//! `derives/get_span.rs`.

use proc_macro2::TokenStream;
use quote::quote;

use crate::arms::Arms;
use crate::output::line_break;
use crate::schema::{FieldDef, FieldType, Payload, Schema, Shape, binding_for, snake_case};

pub(crate) fn generate(schema: &Schema) -> TokenStream {
    let visited: Vec<_> = schema.types.iter().filter(|type_def| type_def.is_visited()).collect();
    let impls = visited.iter().map(|type_def| {
        let ty = type_def.anonymous_ty();
        let (inline, body) = match &type_def.shape {
            Shape::Struct(fields) => {
                let used = spanned_fields(schema, fields);
                (quote!(#[inline]), fields_span(&used, &quote!(self.)).statements())
            }
            Shape::Enum(variants) => {
                let mut matched = Arms::default();
                for variant in variants {
                    let variant_ident = variant.ident();
                    match &variant.payload {
                        Payload::Single(FieldType::Span, tokens) => matched.push(
                            &format!("span: {tokens}"),
                            quote!(Self::#variant_ident(span)),
                            quote!(*span),
                        ),
                        Payload::Single(payload, tokens) => {
                            let binding =
                                binding_for(&payload.node().map(snake_case).unwrap_or_default());
                            matched.push(
                                &format!("{binding}: {tokens}"),
                                quote!(Self::#variant_ident(#binding)),
                                quote!(#binding.span()),
                            );
                        }
                        Payload::Named(fields) => {
                            let used = spanned_fields(schema, fields);
                            let bindings: Vec<String> = used
                                .iter()
                                .map(|field| format!("{}: {}", field.name, field.tokens))
                                .collect();
                            let idents = used.iter().map(|field| field.ident());
                            let rest = (used.len() < fields.len()).then(|| quote!(..));
                            matched.push(
                                &bindings.join(", "),
                                quote!(Self::#variant_ident { #(#idents,)* #rest }),
                                fields_span(&used, &quote!()).expr(),
                            );
                        }
                        Payload::Unit => {}
                    }
                }
                let arms = matched.into_tokens();
                (quote!(), quote!(match self { #arms }))
            }
        };
        quote! {
            impl GetSpan for #ty {
                #inline
                fn span(&self) -> Span {
                    #body
                }
            }
        }
    });
    let names = visited.iter().map(|type_def| type_def.name.as_str());
    let imports = schema.imports(names.chain(["GetSpan", "Span"]));
    let separator = line_break();
    quote! {
        #imports
        #(#separator #impls)*
    }
}

/// The fields a node's span comes from: its `span` field if it has one, or
/// every field that has a span.
fn spanned_fields<'schema>(schema: &Schema, fields: &'schema [FieldDef]) -> Vec<&'schema FieldDef> {
    if let Some(own) = fields.iter().find(|field| field.is_own_span()) {
        return vec![own];
    }
    fields.iter().filter(|field| schema.has_span(&field.ty)).collect()
}

/// A span read from fields, as statements that end in it.
struct SpanCode {
    lets: TokenStream,
    value: TokenStream,
}

impl SpanCode {
    fn statements(self) -> TokenStream {
        let Self { lets, value } = self;
        quote!(#lets #value)
    }

    fn expr(self) -> TokenStream {
        if self.lets.is_empty() {
            self.value
        } else {
            let statements = self.statements();
            quote!({ #statements })
        }
    }
}

/// The span of `fields`, each read as `#place<field>`: a `span` field, or
/// every field's span merged, starting from one that is always there, which
/// `Schema::load` ensures exists.
fn fields_span(fields: &[&FieldDef], place: &TokenStream) -> SpanCode {
    if let [own] = fields
        && own.is_own_span()
    {
        let ident = own.ident();
        let value = if place.is_empty() { quote!(*#ident) } else { quote!(#place #ident) };
        return SpanCode { lets: TokenStream::new(), value };
    }
    let always_spanned = fields.iter().find(|field| !matches!(field.ty, FieldType::Option(_)));
    #[expect(clippy::unreachable, reason = "`Schema::load` checks every node has such a field")]
    let Some(first) = always_spanned else {
        unreachable!("a node without a `span` has a field that always has one");
    };
    let first_ident = first.ident();
    let mut value = quote!(#place #first_ident.span());
    let mut lets = TokenStream::new();
    for field in fields.iter().filter(|field| field.name != first.name) {
        let ident = field.ident();
        if let FieldType::Option(inner) = &field.ty {
            let present = binding_for(&inner.node().map(snake_case).unwrap_or_default());
            let target = if place.is_empty() { quote!(#ident) } else { quote!(&#place #ident) };
            lets.extend(quote! {
                if let Some(#present) = #target {
                    span = span.merge(#present.span());
                }
            });
        } else {
            value = quote!(#value.merge(#place #ident.span()));
        }
    }
    if lets.is_empty() {
        SpanCode { lets, value }
    } else {
        SpanCode { lets: quote!(let mut span = #value; #lets), value: quote!(span) }
    }
}
