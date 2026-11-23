//! `Visit` and `VisitMut`, with a `visit_*` hook and a `walk_*` method for
//! every type the tree visits. Ported from oxc's `generators/visit.rs`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::arms::Arms;
use crate::output::{doc, line_break};
use crate::schema::{
    FieldDef, FieldType, Payload, Schema, Shape, TypeDef, binding_for, snake_case,
};

/// Above this many visits, a walk isn't marked `#[inline]`, as oxc's
/// `INLINE_LIMIT` decides.
const INLINE_LIMIT: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Shared,
    Mutable,
}

impl Access {
    fn reference(self) -> TokenStream {
        match self {
            Self::Shared => quote!(&'ast),
            Self::Mutable => quote!(&mut),
        }
    }

    fn borrow(self) -> TokenStream {
        match self {
            Self::Shared => quote!(&),
            Self::Mutable => quote!(&mut),
        }
    }
}

pub(crate) fn generate_visit(schema: &Schema) -> TokenStream {
    let description = "\
A walk over a tree that borrows it.

Each `visit_*` hook defaults to its `walk_*`, which enters the node, visits
what it holds in source order, and leaves it. Override the hooks you need and
call `self.walk_*` from them to keep recursing. The `walk_*` methods are the
default recursion, so overriding one instead of its hook would leave nothing
to fall back on.

Every node comes with the tree's own lifetime, so a visitor can keep the
references it's handed. A tree lent for less than its arena's lifetime reads
as one of the shorter lifetime, since every node is covariant.";
    generate(schema, Access::Shared, description)
}

pub(crate) fn generate_visit_mut(schema: &Schema) -> TokenStream {
    let description = "\
[`Visit`](crate::Visit)'s walk through `&mut`, in the same order and with the
same levels, for passes that rewrite the tree in place.

Override a `visit_*` hook and call `self.walk_*` from it to recurse. Work
before the call sees the node on the way down, and work after sees it once
what it holds is rewritten, so one hook can run any number of small rewrites
on a node in one walk.

A rewrite edits the node it's handed. One that restructures it moves the parts
it keeps out with `take`, whose placeholder costs no allocation, and builds the
new nodes in the tree's arena, which the pass holds. A rewrite needs no
parentheses for grouping, since the printers write those. One that leaves a
chain as a chain's prefix joins them with
[`SuffixedExpression::splice_prefix`], and one that leaves a run where it
groups as the run around it would can drop the parentheses that prints with
[`BinaryOp::splice_operands`].";
    generate(schema, Access::Mutable, description)
}

fn generate(schema: &Schema, access: Access, description: &str) -> TokenStream {
    let visited: Vec<&TypeDef> =
        schema.types.iter().filter(|type_def| type_def.is_visited()).collect();
    let reference = access.reference();
    let visit_methods = visited.iter().map(|type_def| {
        let (visit, walk, binding, ty) =
            (type_def.visit_ident(), type_def.walk_ident(), type_def.binding(), type_def.ty());
        quote! {
            #[inline]
            fn #visit(&mut self, #binding: #reference #ty) {
                self.#walk(#binding);
            }
        }
    });
    let walk_methods = visited.iter().map(|type_def| match &type_def.shape {
        Shape::Struct(fields) => walk_struct(schema, access, type_def, fields),
        Shape::Enum(_) => walk_enum(schema, access, type_def),
    });
    let (trait_name, kind_ty, enter_text) = match access {
        Access::Shared => (
            "Visit",
            quote!(AstKind<'ast>),
            "Called on entering each node, before what it holds, and always paired with \
             [`Visit::leave_node`].",
        ),
        Access::Mutable => (
            "VisitMut",
            quote!(AstType),
            "Called on entering each node, with its type, before what it holds, and always \
             paired with [`VisitMut::leave_node`].",
        ),
    };
    let trait_ident = format_ident!("{trait_name}");
    let descend_text = format!(
        "Called by the walkers on entering each level of nesting, and always paired with \
         [`{trait_name}::ascend`]. Returning `false` skips what the level holds.\n\n{}",
        levels_doc(schema)
    );
    let names = visited.iter().map(|type_def| type_def.name.as_str());
    let kind_name = if access == Access::Shared { "AstKind" } else { "AstType" };
    let mut imported: Vec<&str> = names.chain([kind_name]).collect();
    if access == Access::Mutable {
        imported.extend(["BinaryOp", "SuffixedExpression"]);
    }
    imported.sort_unstable();
    imported.dedup();
    let imports = schema.imports(imported);
    let separator = line_break();
    let (trait_doc, enter_doc) = (doc(description), doc(enter_text));
    let descend_doc = doc(&descend_text);
    quote! {
        #imports
        #trait_doc
        pub trait #trait_ident<'ast> {
            #enter_doc
            #[inline]
            #[expect(unused_variables, reason = "the default walk needs nothing from a node")]
            fn enter_node(&mut self, kind: #kind_ty) {}
            #separator
            #[inline]
            #[expect(unused_variables, reason = "the default walk needs nothing from a node")]
            fn leave_node(&mut self, kind: #kind_ty) {}
            #separator
            #descend_doc
            #[inline]
            fn descend(&mut self) -> bool {
                true
            }
            #separator
            #[inline]
            fn ascend(&mut self) {}
            #(#separator #visit_methods)*
            #(#separator #walk_methods)*
        }
    }
}

/// The paragraph on `descend` that names every level.
fn levels_doc(schema: &Schema) -> String {
    let mut levels: Vec<String> = Vec::new();
    for type_def in &schema.types {
        if type_def.is_level {
            levels.push(format!("[`{}`]", type_def.name));
        }
        if let Shape::Struct(fields) = &type_def.shape {
            for field in fields.iter().filter(|field| field.is_level) {
                levels.push(format!("[`{}::{}`]", type_def.name, field.name));
            }
        }
    }
    let listed = match levels.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, before)) => format!("{}, or {last}", before.join(", ")),
        None => String::new(),
    };
    format!(
        "A level is the walk of a {listed}. These are the levels \
         [`MAX_NESTING_DEPTH`](crate::MAX_NESTING_DEPTH) caps, so however long a run of \
         suffixes or of binary operators gets, it's one level."
    )
}

fn walk_struct(
    schema: &Schema,
    access: Access,
    type_def: &TypeDef,
    fields: &[FieldDef],
) -> TokenStream {
    let (walk, ident, ty) = (type_def.walk_ident(), type_def.ident(), type_def.ty());
    let binding = type_def.binding();
    let borrow = access.borrow();
    let visits: Vec<TokenStream> = fields
        .iter()
        .filter(|field| schema.visits(&field.ty))
        .map(|field| {
            let field_ident = field.ident();
            let target = quote!(#borrow #binding.#field_ident);
            statements(&visit_field(schema, field, &target))
        })
        .collect();
    let body = if type_def.is_level {
        quote! {
            if self.descend() {
                #(#visits)*
            }
            self.ascend();
        }
    } else {
        quote!(#(#visits)*)
    };
    let kind = match access {
        Access::Shared => quote!(AstKind::#ident(#binding)),
        Access::Mutable => quote!(AstType::#ident),
    };
    let is_unused = access == Access::Mutable && visits.is_empty();
    let unused = is_unused
        .then(|| quote!(#[expect(unused_variables, reason = "nothing it holds is a node")]));
    let inline = (visits.len() <= INLINE_LIMIT).then(|| quote!(#[inline]));
    let reference = access.reference();
    quote! {
        #inline
        #unused
        fn #walk(&mut self, #binding: #reference #ty) {
            let kind = #kind;
            self.enter_node(kind);
            #body
            self.leave_node(kind);
        }
    }
}

fn walk_enum(schema: &Schema, access: Access, type_def: &TypeDef) -> TokenStream {
    let Shape::Enum(variants) = &type_def.shape else {
        return TokenStream::new();
    };
    let (walk, ident, ty) = (type_def.walk_ident(), type_def.ident(), type_def.ty());
    let binding = type_def.binding();
    let mut matched = Arms::default();
    for variant in variants {
        let variant_ident = variant.ident();
        match &variant.payload {
            Payload::Unit => matched.push("", quote!(#ident::#variant_ident), quote!({})),
            Payload::Single(payload, tokens) if schema.visits(payload) => {
                let snake = payload.node().map(snake_case).unwrap_or_default();
                let payload_binding = binding_for(&snake);
                let visits = visit_value(schema, payload, &quote!(#payload_binding), &snake);
                matched.push(
                    &format!("{payload_binding}: {tokens}"),
                    quote!(#ident::#variant_ident(#payload_binding)),
                    arm_body(&visits),
                );
            }
            Payload::Single(..) => matched.push("", quote!(#ident::#variant_ident(_)), quote!({})),
            Payload::Named(fields) => {
                let bound: Vec<&FieldDef> =
                    fields.iter().filter(|field| schema.visits(&field.ty)).collect();
                let idents = bound.iter().map(|field| field.ident());
                let rest = (bound.len() < fields.len()).then(|| quote!(..));
                let pattern = quote!(#ident::#variant_ident { #(#idents,)* #rest });
                let visits: Vec<Visit> = bound
                    .iter()
                    .flat_map(|field| {
                        let field_ident = field.ident();
                        visit_field(schema, field, &quote!(#field_ident))
                    })
                    .collect();
                let bindings: Vec<String> =
                    bound.iter().map(|field| format!("{}: {}", field.name, field.tokens)).collect();
                matched.push(&bindings.join(", "), pattern, arm_body(&visits));
            }
        }
    }
    let arms = matched.into_tokens();
    let reference = access.reference();
    quote! {
        fn #walk(&mut self, #binding: #reference #ty) {
            match #binding {
                #arms
            }
        }
    }
}

/// One step of a walk: a call, which a statement ends with `;`, or a block.
struct Visit {
    tokens: TokenStream,
    is_call: bool,
}

fn statements(visits: &[Visit]) -> TokenStream {
    let statements = visits.iter().map(|visit| {
        let tokens = &visit.tokens;
        if visit.is_call { quote!(#tokens;) } else { quote!(#tokens) }
    });
    quote!(#(#statements)*)
}

fn arm_body(visits: &[Visit]) -> TokenStream {
    match visits {
        [visit] if visit.is_call => visit.tokens.clone(),
        _ => {
            let statements = statements(visits);
            quote!({ #statements })
        }
    }
}

/// The visit of `field` at `target`, inside a level of its own when it's
/// marked one.
fn visit_field(schema: &Schema, field: &FieldDef, target: &TokenStream) -> Vec<Visit> {
    if !field.is_level {
        return visit_value(schema, &field.ty, target, &field.name);
    }
    let in_level = |visits: &[Visit]| {
        let statements = statements(visits);
        quote! {
            if self.descend() {
                #statements
            }
            self.ascend();
        }
    };
    let tokens = if let FieldType::Option(inner) = &field.ty {
        let present = binding_for(&field.name);
        let level = in_level(&visit_value(schema, inner, &quote!(#present), &field.name));
        quote! {
            if let Some(#present) = #target {
                #level
            }
        }
    } else {
        let level = in_level(&visit_value(schema, &field.ty, target, &field.name));
        quote!({ #level })
    };
    vec![Visit { tokens, is_call: false }]
}

/// The visits of what `ty` at `target` holds. An `Option` that holds a value
/// binds it as `name`.
fn visit_value(schema: &Schema, ty: &FieldType, target: &TokenStream, name: &str) -> Vec<Visit> {
    match ty {
        FieldType::Node(node) => schema
            .get(node)
            .map(|type_def| {
                let visit = type_def.visit_ident();
                Visit { tokens: quote!(self.#visit(#target)), is_call: true }
            })
            .into_iter()
            .collect(),
        FieldType::Box(inner) => visit_value(schema, inner, target, name),
        FieldType::List(inner) | FieldType::OneOrMore(inner) => {
            let snake = inner.node().map(snake_case).unwrap_or_default();
            let item = binding_for(&snake);
            let body = statements(&visit_value(schema, inner, &quote!(#item), &snake));
            vec![Visit { tokens: quote!(for #item in #target { #body }), is_call: false }]
        }
        FieldType::Option(inner) => {
            let present = binding_for(name);
            let body = statements(&visit_value(schema, inner, &quote!(#present), name));
            let tokens = quote!(if let Some(#present) = #target { #body });
            vec![Visit { tokens, is_call: false }]
        }
        FieldType::Span | FieldType::Other => Vec::new(),
    }
}
