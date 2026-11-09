use core::ops::ControlFlow::{self, Continue};

use luck_ast::{
    FunctionType, FunctionTypeParam, GenericTypeList, GenericTypeParam, NamedType, TableType, Type,
    TypeArgs, TypeField, ensure_sufficient_stack,
};
use luck_token::{LuaVersion, Token, TokenKind};

use super::expr::expr_eq;
use super::{
    AstDiff, at, at_index, differ, presence_eq, punctuated_eq, require, string_raw_eq,
    token_text_eq,
};

/// Compare optional `: Type` annotations. A dropped annotation is data loss,
/// so presence has to match too.
pub(super) fn type_annotation_eq(
    left: Option<&Type>,
    right: Option<&Type>,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    presence_eq(left, right, "type annotation presence differs", |lhs, rhs| {
        type_eq(lhs, rhs, version)
    })
}

/// Structural equivalence of two Luau types, tolerating the formatter's
/// semantics-neutral rewrites (redundant parens, leading union/intersection
/// separators, and `,`/`;` table-field separators).
pub(super) fn type_eq(left: &Type, right: &Type, version: LuaVersion) -> ControlFlow<AstDiff> {
    // As with expression parens in `expr_eq`, the formatter may add or drop
    // redundant type parens.
    let left_ty = unwrap_type_parens(left);
    let right_ty = unwrap_type_parens(right);
    match (left_ty, right_ty) {
        // Unreachable after `unwrap_type_parens`, but recursing is safer than
        // panicking.
        (Type::Parenthesized(inner), _) => type_eq(&inner.type_value, right_ty, version),
        (Type::Name { name: lhs, .. }, Type::Name { name: rhs, .. }) => {
            at("name", require(lhs == rhs, "type name differs"))
        }
        (Type::Name { name, .. }, Type::Named(named))
        | (Type::Named(named), Type::Name { name, .. }) => simple_named_type_eq(name, named),
        (Type::Named(lhs), Type::Named(rhs)) => named_type_eq(lhs, rhs, version),
        (Type::Typeof(lhs), Type::Typeof(rhs)) => {
            at("Typeof/expr", expr_eq(&lhs.expr, &rhs.expr, version))
        }
        (Type::Table(lhs), Type::Table(rhs)) => table_type_eq(lhs, rhs, version),
        (Type::Function(lhs), Type::Function(rhs)) => function_type_eq(lhs, rhs, version),
        (Type::Optional(lhs), Type::Optional(rhs)) => {
            ensure_sufficient_stack(|| type_eq(&lhs.type_value, &rhs.type_value, version))
        }
        // `leading_pipe` is trivia the formatter may add or remove.
        (Type::Union(lhs), Type::Union(rhs)) => {
            at("Union/types", punctuated_eq(&lhs.types, &rhs.types, version, type_eq))
        }
        // `leading_ampersand` is trivia the formatter may add or remove.
        (Type::Intersection(lhs), Type::Intersection(rhs)) => {
            at("Intersection/types", punctuated_eq(&lhs.types, &rhs.types, version, type_eq))
        }
        (Type::Pack(lhs), Type::Pack(rhs)) => {
            at("Pack/types", punctuated_eq(&lhs.types, &rhs.types, version, type_eq))
        }
        (Type::Singleton(lhs), Type::Singleton(rhs)) => {
            require(singleton_eq(lhs, rhs, version), "singleton type differs")
        }
        (Type::Variadic(lhs), Type::Variadic(rhs)) => {
            type_eq(&lhs.type_value, &rhs.type_value, version)
        }
        // The trailing `...` is fixed syntax; only the pack name varies.
        (Type::GenericPack(lhs), Type::GenericPack(rhs)) => token_text_eq(&lhs.name, &rhs.name),
        (Type::Error(_), Type::Error(_)) => Continue(()),
        (
            Type::Name { .. }
            | Type::Named(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Function(_)
            | Type::Optional(_)
            | Type::Union(_)
            | Type::Intersection(_)
            | Type::Pack(_)
            | Type::Singleton(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_),
            _,
        ) => differ(format!(
            "type kind differs: `{}` vs `{}`",
            type_kind(left_ty),
            type_kind(right_ty)
        )),
    }
}

/// Strip any nesting of redundant type parens down to the inner type.
fn unwrap_type_parens(ty: &Type) -> &Type {
    let mut current = ty;
    while let Type::Parenthesized(inner) = current {
        current = &inner.type_value;
    }
    current
}

fn named_type_eq(left: &NamedType, right: &NamedType, version: LuaVersion) -> ControlFlow<AstDiff> {
    presence_eq(
        left.prefix.as_ref(),
        right.prefix.as_ref(),
        "named type module presence differs",
        |lhs, rhs| at("prefix", token_text_eq(lhs, rhs)),
    )?;
    at("name", token_text_eq(&left.name, &right.name))?;
    at("generics", type_args_eq(left.generics.as_ref(), right.generics.as_ref(), version))
}

fn type_args_eq(
    left: Option<&TypeArgs>,
    right: Option<&TypeArgs>,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    presence_eq(left, right, "type argument list presence differs", |lhs, rhs| {
        punctuated_eq(&lhs.args, &rhs.args, version, type_eq)
    })
}

fn table_type_eq(left: &TableType, right: &TableType, version: LuaVersion) -> ControlFlow<AstDiff> {
    if left.fields.len() != right.fields.len() {
        return differ(format!(
            "table type field count differs: {} vs {}",
            left.fields.len(),
            right.fields.len()
        ));
    }
    // Field separators (`,` or `;`) are trivia; only the fields matter.
    for (index, (lhs, rhs)) in left.fields.iter().zip(right.fields.iter()).enumerate() {
        at_index("fields", index, type_field_eq(lhs, rhs, version))?;
    }
    Continue(())
}

fn type_field_eq(left: &TypeField, right: &TypeField, version: LuaVersion) -> ControlFlow<AstDiff> {
    match (left, right) {
        (
            TypeField::Named { access: left_access, name: left_name, value: left_value, .. },
            TypeField::Named { access: right_access, name: right_name, value: right_value, .. },
        ) => {
            at("access", access_modifier_eq(left_access.as_ref(), right_access.as_ref()))?;
            at("name", token_text_eq(left_name, right_name))?;
            at("value", type_eq(left_value, right_value, version))
        }
        (
            TypeField::Indexer { access: left_access, key: left_key, value: left_value, .. },
            TypeField::Indexer { access: right_access, key: right_key, value: right_value, .. },
        ) => {
            at("access", access_modifier_eq(left_access.as_ref(), right_access.as_ref()))?;
            at("key", type_eq(left_key, right_key, version))?;
            at("value", type_eq(left_value, right_value, version))
        }
        (
            TypeField::Array { access: left_access, value: left_value, .. },
            TypeField::Array { access: right_access, value: right_value, .. },
        ) => {
            at("access", access_modifier_eq(left_access.as_ref(), right_access.as_ref()))?;
            type_eq(left_value, right_value, version)
        }
        (TypeField::Named { .. } | TypeField::Indexer { .. } | TypeField::Array { .. }, _) => {
            differ("table type field kind differs")
        }
    }
}

/// `read`/`write` modifiers are semantic, so presence and text have to match.
fn access_modifier_eq(left: Option<&Token>, right: Option<&Token>) -> ControlFlow<AstDiff> {
    presence_eq(left, right, "access modifier presence differs", token_text_eq)
}

fn function_type_eq(
    left: &FunctionType,
    right: &FunctionType,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("generics", generics_eq(left.generics.as_ref(), right.generics.as_ref(), version))?;
    at("params", punctuated_eq(&left.params, &right.params, version, function_type_param_eq))?;
    at("return_type", type_eq(&left.return_type, &right.return_type, version))
}

fn function_type_param_eq(
    left: &FunctionTypeParam,
    right: &FunctionTypeParam,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    // Function-type param names are documentation in Luau, but the formatter
    // never drops them, so compare presence and text strictly.
    presence_eq(
        left.name.as_ref(),
        right.name.as_ref(),
        "function type parameter name presence differs",
        |lhs, rhs| at("name", token_text_eq(lhs, rhs)),
    )?;
    type_eq(&left.type_value, &right.type_value, version)
}

/// Takes `Option<&_>` so the boxed (`FunctionBody`, `TypeDeclaration`) and
/// unboxed (`FunctionType`) generic lists share one comparison.
pub(super) fn generics_eq(
    left: Option<&GenericTypeList>,
    right: Option<&GenericTypeList>,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    presence_eq(left, right, "generic parameter list presence differs", |lhs, rhs| {
        punctuated_eq(&lhs.params, &rhs.params, version, generic_param_eq)
    })
}

fn generic_param_eq(
    left: &GenericTypeParam,
    right: &GenericTypeParam,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    token_text_eq(&left.name, &right.name)?;
    require(left.is_pack == right.is_pack, "generic pack marker presence differs")?;
    presence_eq(
        left.default.as_ref(),
        right.default.as_ref(),
        "generic default presence differs",
        |lhs, rhs| at("default", type_eq(lhs, rhs, version)),
    )
}

/// A bare `Type::Name` against a `NamedType`, reporting at the paths
/// `named_type_eq` uses.
fn simple_named_type_eq(name: &str, named: &NamedType) -> ControlFlow<AstDiff> {
    if named.prefix.is_some() {
        return differ("named type module presence differs");
    }
    if !matches!(&named.name.kind, TokenKind::Identifier(other) if other == name) {
        return at("name", differ("type name differs"));
    }
    if named.generics.is_some() {
        return at("generics", differ("type argument list presence differs"));
    }
    Continue(())
}

/// String singletons compare by decoded content, so a quote-style swap still
/// matches; the rest compare by kind.
fn singleton_eq(left: &Token, right: &Token, version: LuaVersion) -> bool {
    match (&left.kind, &right.kind) {
        (TokenKind::StringLiteral(left_raw), TokenKind::StringLiteral(right_raw)) => {
            string_raw_eq(left_raw, right_raw, version)
        }
        _ => left.kind == right.kind,
    }
}

const fn type_kind(ty: &Type) -> &'static str {
    match ty {
        Type::Name { .. } | Type::Named(_) => "Named",
        Type::Typeof(_) => "Typeof",
        Type::Table(_) => "Table",
        Type::Function(_) => "Function",
        Type::Optional(_) => "Optional",
        Type::Union(_) => "Union",
        Type::Intersection(_) => "Intersection",
        Type::Parenthesized(_) => "Parenthesized",
        Type::Pack(_) => "Pack",
        Type::Singleton(_) => "Singleton",
        Type::Variadic(_) => "Variadic",
        Type::GenericPack(_) => "GenericPack",
        Type::Error(_) => "Error",
    }
}

#[cfg(test)]
mod tests {
    use luck_ast::Synth;

    use super::*;

    #[test]
    fn compact_and_general_type_names_are_equivalent() {
        let synth = Synth::new();
        let compact = synth.ty_named("number");
        let general = Type::Named(Box::new(NamedType {
            span: luck_token::Span::new(0, 0),
            prefix: None,
            name: synth.identifier("number"),
            generics: None,
        }));
        let version = LuaVersion::Luau;
        assert!(type_eq(&compact, &general, version).is_continue());
        assert!(type_eq(&general, &compact, version).is_continue());
        for different in [
            synth.ty_named("string"),
            synth.ty_qualified("module", "number"),
            synth.ty_generic("number", vec![synth.ty_named("T")]),
        ] {
            assert!(type_eq(&compact, &different, version).is_break());
            assert!(type_eq(&different, &compact, version).is_break());
        }
    }
}
