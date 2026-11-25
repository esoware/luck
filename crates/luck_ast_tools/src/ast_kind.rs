//! `AstKind`, a reference to any node, `AstType`, which kind it is, one
//! variant for each struct that derives `Ast` and has a kind, and
//! `AST_TYPE_MAX`. Ported from oxc's `generators/ast_kind.rs`.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::output::{doc, line_break};
use crate::schema::Schema;

pub(crate) fn generate(schema: &Schema) -> TokenStream {
    let nodes: Vec<_> = schema
        .structs()
        .map(|(type_def, _)| type_def)
        .filter(|type_def| type_def.has_kind)
        .collect();
    let idents: Vec<_> = nodes.iter().map(|type_def| type_def.ident()).collect();
    let types: Vec<_> = nodes.iter().map(|type_def| type_def.ty()).collect();
    let bindings: Vec<_> = nodes.iter().map(|type_def| type_def.binding()).collect();
    let as_methods = nodes.iter().map(|type_def| {
        let (ident, ty, binding) = (type_def.ident(), type_def.ty(), type_def.binding());
        let method = format_ident!("as_{}", type_def.snake_name());
        quote! {
            #[inline]
            pub const fn #method(self) -> Option<&'ast #ty> {
                if let Self::#ident(#binding) = self { Some(#binding) } else { None }
            }
        }
    });
    let imports = schema
        .imports(nodes.iter().map(|type_def| type_def.name.as_str()).chain(["GetSpan", "Span"]));
    let separator = line_break();
    let ast_type_max = Literal::u8_unsuffixed(
        u8::try_from(nodes.len() - 1).expect("`AstType` is `repr(u8)`, so its discriminants fit"),
    );
    let max_doc = doc("The largest discriminant of an [`AstType`].");
    let type_doc = doc("Which kind of node an [`AstKind`] refers to.");
    let kind_doc = doc("A reference to a node: a value of a struct that derives `Ast` without \
         `#[visit(no_kind)]`, which [`Visit::enter_node`](crate::Visit::enter_node) is handed.");
    quote! {
        #imports
        #max_doc
        pub const AST_TYPE_MAX: u8 = #ast_type_max;
        #separator
        #type_doc
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum AstType {
            #(#idents,)*
        }
        #separator
        #kind_doc
        #[derive(Debug, Clone, Copy)]
        pub enum AstKind<'ast> {
            #(#idents(&'ast #types),)*
        }
        #separator
        impl<'ast> AstKind<'ast> {
            #[inline]
            pub const fn ty(self) -> AstType {
                match self {
                    #(Self::#idents(_) => AstType::#idents,)*
                }
            }
            #(#separator #as_methods)*
        }
        #separator
        impl GetSpan for AstKind<'_> {
            fn span(&self) -> Span {
                match self {
                    #(Self::#idents(#bindings) => #bindings.span(),)*
                }
            }
        }
    }
}
