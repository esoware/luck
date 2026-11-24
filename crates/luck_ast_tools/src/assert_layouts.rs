//! A test that pins the size and alignment of every type that derives `Ast`
//! and of every `Option` a field holds, against a committed snapshot, so a
//! node can't grow unnoticed.
//!
//! oxc's `generators/assert_layouts.rs` computes the layouts itself, which
//! works because `#[ast]` makes every oxc node `#[repr(C)]`. luck's nodes
//! keep the compiler's layout, which packs them tighter, so the compiler
//! measures them and the snapshot records what it found.

use std::collections::BTreeSet;

use proc_macro2::{Literal, TokenStream};
use quote::quote;

use crate::output::line_break;
use crate::schema::{FieldType, Schema, type_names};

pub(crate) fn generate(schema: &Schema) -> TokenStream {
    let mut rows: Vec<(String, TokenStream)> = schema
        .types
        .iter()
        .map(|type_def| (type_def.name.clone(), type_def.anonymous_ty()))
        .collect();
    let mut names: BTreeSet<String> =
        schema.types.iter().map(|type_def| type_def.name.clone()).collect();
    let mut options = BTreeSet::new();
    for (ty, tokens) in schema.field_types_with_tokens() {
        if let FieldType::Option(inner) = ty
            && !matches!(**inner, FieldType::Box(_))
        {
            let written = tokens.to_string();
            if options.insert(written.clone()) {
                type_names(tokens, &mut names);
                let label = written
                    .replace("'ast ,", "")
                    .replace("'ast", "")
                    .replace(' ', "")
                    .replace("<>", "");
                let anonymous =
                    written.replace("'ast", "'_").parse().expect("a field type reparses");
                rows.push((label, anonymous));
            }
        }
    }
    let labels: Vec<&String> = rows.iter().map(|(label, _)| label).collect();
    let types: Vec<&TokenStream> = rows.iter().map(|(_, ty)| ty).collect();
    let count = Literal::usize_unsuffixed(rows.len());
    let imports = schema.imports(names.iter().map(String::as_str));
    let separator = line_break();
    quote! {
        #imports
        const LAYOUTS: [(&str, usize, usize); #count] =
            [#((#labels, size_of::<#types>(), align_of::<#types>()),)*];
        #separator
        #[test]
        fn layouts() {
            let table = LAYOUTS
                .map(|(name, size, align)| format!("{name}: size {size}, align {align}"))
                .join("\n");
            insta::assert_snapshot!(table);
        }
    }
}
