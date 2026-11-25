//! `ContentEq` for every type that derives `Ast`: equality that leaves out
//! spans, a name's index, and the fields marked `#[content_eq(skip)]`.
//! Ported from oxc's `derives/content_eq.rs`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::arms::Arms;
use crate::output::line_break;
use crate::schema::{FieldDef, FieldType, Payload, Schema, Shape, VariantDef};

pub(crate) fn generate(schema: &Schema) -> TokenStream {
    let impls = schema.types.iter().map(|type_def| {
        let ty = type_def.anonymous_ty();
        let (uses_other, body) = match &type_def.shape {
            Shape::Struct(fields) => {
                let compared: Vec<_> = compared_fields(fields).collect();
                if compared.is_empty() {
                    (false, quote!(true))
                } else {
                    let idents = compared.iter().map(|field| field.ident());
                    (true, quote!(#(ContentEq::content_eq(&self.#idents, &other.#idents))&&*))
                }
            }
            Shape::Enum(_) if !type_def.is_visited() => (true, quote!(self == other)),
            Shape::Enum(variants) => (true, enum_body(variants)),
        };
        let other = if uses_other { format_ident!("other") } else { format_ident!("_other") };
        quote! {
            impl ContentEq for #ty {
                fn content_eq(&self, #other: &Self) -> bool {
                    #body
                }
            }
        }
    });
    let names = schema.types.iter().map(|type_def| type_def.name.as_str());
    let imports = schema.imports(names.chain(["ContentEq"]));
    let separator = line_break();
    quote! {
        #imports
        #(#separator #impls)*
    }
}

/// The fields `ContentEq` compares: every one but a span and those marked
/// `#[content_eq(skip)]`.
fn compared_fields(fields: &[FieldDef]) -> impl Iterator<Item = &FieldDef> {
    fields.iter().filter(|field| field.ty != FieldType::Span && !field.skips_content_eq)
}

/// A `match` on both values: a variant equals the same variant with equal
/// contents, and no other.
fn enum_body(variants: &[VariantDef]) -> TokenStream {
    let mut matched = Arms::default();
    let mut any_of_each = Vec::new();
    for variant in variants {
        let ident = variant.ident();
        match &variant.payload {
            Payload::Unit => {
                matched.push("", quote!((Self::#ident, Self::#ident)), quote!(true));
                any_of_each.push(quote!(Self::#ident));
            }
            Payload::Single(FieldType::Span, _) => {
                matched.push("", quote!((Self::#ident(_), Self::#ident(_))), quote!(true));
                any_of_each.push(quote!(Self::#ident(_)));
            }
            Payload::Single(_, tokens) => {
                matched.push(
                    &tokens.to_string(),
                    quote!((Self::#ident(lhs), Self::#ident(rhs))),
                    quote!(ContentEq::content_eq(lhs, rhs)),
                );
                any_of_each.push(quote!(Self::#ident(_)));
            }
            Payload::Named(fields) => {
                let compared: Vec<&FieldDef> = compared_fields(fields).collect();
                let names: Vec<_> = compared.iter().map(|field| field.ident()).collect();
                let lhs: Vec<_> = names.iter().map(|name| format_ident!("lhs_{name}")).collect();
                let rhs: Vec<_> = names.iter().map(|name| format_ident!("rhs_{name}")).collect();
                let rest = (compared.len() < fields.len()).then(|| quote!(..));
                let pattern = quote! {(
                    Self::#ident { #(#names: #lhs,)* #rest },
                    Self::#ident { #(#names: #rhs,)* #rest },
                )};
                let body = if compared.is_empty() {
                    quote!(true)
                } else {
                    quote!(#(ContentEq::content_eq(#lhs, #rhs))&&*)
                };
                let bindings: Vec<String> = compared
                    .iter()
                    .map(|field| format!("{}: {}", field.name, field.tokens))
                    .collect();
                matched.push(&bindings.join(", "), pattern, body);
                any_of_each.push(quote!(Self::#ident { .. }));
            }
        }
    }
    let arms = matched.into_tokens();
    let mismatch = (variants.len() > 1).then(|| quote!((#(#any_of_each)|*, _) => false,));
    quote! {
        match (self, other) {
            #arms
            #mismatch
        }
    }
}
