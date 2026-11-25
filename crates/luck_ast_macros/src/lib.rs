//! # `luck_ast_macros`
//!
//! `#[derive(Ast)]` marks a type as part of `luck_ast`'s syntax tree and
//! generates nothing. `luck_ast_tools` reads the marked types and writes the
//! tree's visitors, node kinds, span accessors, content equality, builder,
//! and layout assertions from them. The derive exists so the marked types
//! can carry the tool's attributes:
//!
//! - `#[visit(level)]` on a type makes its walk one level of nesting, and on
//!   a field makes visiting that field one, as `Visit::descend` counts them.
//! - `#[visit(no_kind)]` on a struct makes it no node of its own, with no
//!   `AstKind`, for a struct that spans exactly what it holds.
//! - `#[content_eq(skip)]` on a field leaves it out of `ContentEq`, for a
//!   field that records only how the source was written.
//!
//! Ported from `oxc_ast_macros`' `Ast` derive.

use proc_macro::TokenStream;

#[proc_macro_derive(Ast, attributes(visit, content_eq))]
pub fn derive_ast(_item: TokenStream) -> TokenStream {
    TokenStream::new()
}
