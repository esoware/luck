//! The tree's low-level builder: a constructor on every node type, with no
//! policy, that takes each field in order and the arena last.
//!
//! - `new` builds a struct, and `boxed` builds one in the arena, for a
//!   struct a field holds in a `Box`.
//! - `new_*` builds an enum variant that holds a struct from that struct's
//!   fields. A variant that holds anything else is built as itself.
//!
//! Ported from oxc's `generators/ast_builder.rs`, which puts its builder
//! methods on the AST types too.

use std::collections::BTreeSet;

use proc_macro2::{Group, TokenStream, TokenTree};
use quote::{format_ident, quote};

use crate::output::line_break;
use crate::schema::{FieldDef, FieldType, Payload, Schema, Shape, TypeDef, snake_case, type_names};

/// Past this many parameters, clippy's `too_many_arguments` needs an
/// expectation.
const MAX_ARGUMENTS: usize = 7;

pub(crate) fn generate(schema: &Schema) -> TokenStream {
    let boxed = schema.boxed_structs();
    let mut names: BTreeSet<String> = ["Allocator", "Box"].map(str::to_owned).into();
    let separator = line_break();
    let typed_impls = schema.types.iter().filter_map(|type_def| {
        let methods: Vec<TokenStream> = match &type_def.shape {
            Shape::Struct(fields) => {
                struct_methods(type_def, fields, boxed.contains(type_def.name.as_str()))
            }
            Shape::Enum(variants) => variants
                .iter()
                .filter_map(|variant| {
                    let Payload::Single(payload, _) = &variant.payload else {
                        return None;
                    };
                    let (held, is_boxed) = match payload {
                        FieldType::Box(boxed_payload) => (&**boxed_payload, true),
                        FieldType::Span
                        | FieldType::Node(_)
                        | FieldType::List(_)
                        | FieldType::OneOrMore(_)
                        | FieldType::Option(_)
                        | FieldType::Other => (payload, false),
                    };
                    let FieldType::Node(name) = held else {
                        return None;
                    };
                    let inner = schema.get(name).expect("every node is in the schema");
                    let Shape::Struct(fields) = &inner.shape else {
                        return None;
                    };
                    Some(variant_method(type_def, &variant.name, inner, fields, is_boxed))
                })
                .collect(),
        };
        if methods.is_empty() {
            return None;
        }
        names.insert(type_def.name.clone());
        let (generics, ty) = (lifetime(type_def), type_def.ty());
        Some(quote! {
            impl #generics #ty {
                #(#separator #methods)*
            }
        })
    });
    let impls: Vec<TokenStream> = typed_impls.collect();
    for (type_def, fields) in schema.structs() {
        names.insert(type_def.name.clone());
        for field in fields {
            type_names(&field.tokens, &mut names);
        }
    }
    let imports = schema.imports(names.iter().map(String::as_str));
    quote! {
        #imports
        #(#separator #impls)*
    }
}

fn lifetime(type_def: &TypeDef) -> TokenStream {
    if type_def.has_lifetime { quote!(<'ast>) } else { TokenStream::new() }
}

/// `new`, and `boxed` when the struct is boxed somewhere.
fn struct_methods(type_def: &TypeDef, fields: &[FieldDef], is_boxed: bool) -> Vec<TokenStream> {
    let idents: Vec<_> = fields.iter().map(FieldDef::ident).collect();
    let types = fields.iter().map(|field| with_self(&field.tokens, &type_def.name));
    let params = quote!(#(#idents: #types),*);
    let too_many = |count: usize| {
        (count > MAX_ARGUMENTS).then(|| {
            quote!(#[expect(clippy::too_many_arguments, reason = "a node takes each of its fields")])
        })
    };
    let new_expectation = too_many(idents.len());
    let mut methods = vec![quote! {
        #[inline]
        #new_expectation
        pub const fn new(#params) -> Self {
            Self { #(#idents),* }
        }
    }];
    if is_boxed {
        let boxed_expectation = too_many(idents.len() + 1);
        let generics = (!type_def.has_lifetime).then(|| quote!(<'ast>));
        methods.push(quote! {
            #[inline]
            #boxed_expectation
            pub fn boxed #generics (#params, allocator: &'ast Allocator) -> Box<'ast, Self> {
                Box::new_in(Self::new(#(#idents),*), allocator)
            }
        });
    }
    methods
}

/// `new_<variant>` on the enum `outer`, which builds the struct `inner` the
/// variant holds from its fields.
fn variant_method(
    outer: &TypeDef,
    variant: &str,
    inner: &TypeDef,
    fields: &[FieldDef],
    is_boxed: bool,
) -> TokenStream {
    let (variant_ident, inner_ident) = (format_ident!("{variant}"), inner.ident());
    let method = format_ident!("new_{}", snake_case(variant));
    let idents: Vec<_> = fields.iter().map(FieldDef::ident).collect();
    let types = fields.iter().map(|field| with_self(&field.tokens, &outer.name));
    let (allocator, build, constness) = if is_boxed {
        (
            quote!(, allocator: &'ast Allocator),
            quote!(#inner_ident::boxed(#(#idents,)* allocator)),
            TokenStream::new(),
        )
    } else {
        (TokenStream::new(), quote!(#inner_ident::new(#(#idents),*)), quote!(const))
    };
    let count = idents.len() + usize::from(is_boxed);
    let expectation = (count > MAX_ARGUMENTS).then(|| {
        quote!(#[expect(clippy::too_many_arguments, reason = "a node takes each of its fields")])
    });
    quote! {
        #[inline]
        #expectation
        pub #constness fn #method(#(#idents: #types),* #allocator) -> Self {
            Self::#variant_ident(#build)
        }
    }
}

/// `tokens` with the type `name` written as `Self` inside its own `impl`.
/// The type's lifetime is the `impl`'s, so its generics go too.
fn with_self(tokens: &TokenStream, name: &str) -> TokenStream {
    let mut written = TokenStream::new();
    let mut trees = tokens.clone().into_iter().peekable();
    while let Some(tree) = trees.next() {
        match tree {
            TokenTree::Ident(ident) if ident == name => {
                written.extend(quote!(Self));
                if trees.next_if(|next| is_punct(next, '<')).is_some() {
                    let mut depth = 1usize;
                    for skipped in trees.by_ref() {
                        if is_punct(&skipped, '<') {
                            depth += 1;
                        }
                        if is_punct(&skipped, '>') {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                    }
                }
            }
            TokenTree::Group(group) => {
                let mut inner = Group::new(group.delimiter(), with_self(&group.stream(), name));
                inner.set_span(group.span());
                written.extend([TokenTree::Group(inner)]);
            }
            TokenTree::Ident(_) | TokenTree::Punct(_) | TokenTree::Literal(_) => {
                written.extend([tree]);
            }
        }
    }
    written
}

fn is_punct(tree: &TokenTree, c: char) -> bool {
    matches!(tree, TokenTree::Punct(punct) if punct.as_char() == c)
}
