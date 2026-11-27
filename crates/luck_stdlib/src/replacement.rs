//! Which calls of a deprecated function the function its `use` names takes in
//! its place, as far as the types a definition file declares for both say
//! without a type checker, so a type it can't compare takes nothing.

use std::slice;

use luck_ast::{ContentEq as _, DeclaredFunction, FunctionType, GenericTypeList, Type};

use crate::stdlib_model::StdlibReplacement;
use crate::translate::takes_nil;

/// What one param, or each value of a vararg, takes.
#[derive(Debug, Clone, Copy)]
enum Param<'ast> {
    /// `T`, or `...T` for each value of a vararg.
    Typed(&'ast Type<'ast>),
    /// A param with no annotation, or a bare `...`.
    Any,
    /// A vararg of the generic pack `T...`.
    Pack(&'ast str),
}

/// A function's signature as a definition file declares it, without a
/// method's `self`.
#[derive(Debug)]
pub(crate) struct Signature<'ast> {
    params: Vec<Param<'ast>>,
    vararg: Option<Param<'ast>>,
    returns: &'ast [Type<'ast>],
    generic_types: Vec<&'ast str>,
    generic_packs: Vec<&'ast str>,
    /// Through the last param that doesn't take `nil`.
    min_args: usize,
}

impl<'ast> Signature<'ast> {
    pub(crate) fn of_declared(function: &'ast DeclaredFunction<'ast>, is_method: bool) -> Self {
        let params = function
            .params
            .iter()
            .skip(usize::from(is_method))
            .map(|param| param.type_annotation.as_deref().map_or(Param::Any, Param::Typed))
            .collect();
        let vararg =
            function.vararg.as_deref().map(|vararg| match vararg.type_annotation.as_deref() {
                Some(Type::GenericPack(pack)) => Param::Pack(pack.name.text.as_str()),
                Some(ty) => Param::Typed(ty),
                None => Param::Any,
            });
        let returns = returned(function.return_type.as_deref());
        Self::new(params, vararg, returns, function.generics.as_deref())
    }

    pub(crate) fn of_type(function: &'ast FunctionType<'ast>) -> Self {
        let mut params = Vec::new();
        let mut vararg = None;
        for param in &function.params {
            if let Type::Variadic(variadic) = &param.ty {
                vararg = Some(Param::Typed(&variadic.ty));
            } else if let Type::GenericPack(pack) = &param.ty {
                vararg = Some(Param::Pack(pack.name.text.as_str()));
            } else {
                params.push(Param::Typed(&param.ty));
            }
        }
        let returns = returned(Some(&function.return_type));
        Self::new(params, vararg, returns, function.generics.as_deref())
    }

    fn new(
        params: Vec<Param<'ast>>,
        vararg: Option<Param<'ast>>,
        returns: &'ast [Type<'ast>],
        generics: Option<&'ast GenericTypeList<'ast>>,
    ) -> Self {
        let (mut generic_types, mut generic_packs) = (Vec::new(), Vec::new());
        for generic in generics.iter().flat_map(|list| &list.params) {
            let names = if generic.is_pack { &mut generic_packs } else { &mut generic_types };
            names.push(generic.name.text.as_str());
        }
        let min_args = params
            .iter()
            .rposition(|param| matches!(param, Param::Typed(ty) if !takes_nil(ty)))
            .map_or(0, |last| last + 1);
        Self { params, vararg, returns, generic_types, generic_packs, min_args }
    }

    fn takes(&self, arg_count: usize) -> bool {
        arg_count >= self.min_args && (self.vararg.is_some() || arg_count <= self.params.len())
    }

    fn param(&self, index: usize) -> Option<Param<'ast>> {
        self.params.get(index).copied().or(self.vararg)
    }

    /// Whether the param at `index` takes everything `other` does.
    fn param_takes(&self, index: usize, other: Param<'_>) -> bool {
        match (self.param(index), other) {
            (None, _) => false,
            (Some(Param::Any), _) => true,
            (Some(Param::Pack(name)), _) => self.generic_packs.contains(&name),
            (Some(Param::Typed(ty)), Param::Typed(other)) => self.type_takes(ty, other),
            (Some(Param::Typed(ty)), Param::Any | Param::Pack(_)) => self.takes_anything(ty),
        }
    }

    /// Whether a value of type `ty` can be any value: `any`, `unknown`, or
    /// one of the function's generic types.
    fn takes_anything(&self, ty: &Type<'_>) -> bool {
        let Type::Name(name) = unparenthesized(ty) else {
            return false;
        };
        is_top(ty) || self.generic_types.contains(&name.text.as_str())
    }

    /// Whether a value of type `ty` can be every value of type `other`.
    fn type_takes(&self, ty: &Type<'_>, other: &Type<'_>) -> bool {
        let (ty, other) = (unparenthesized(ty), unparenthesized(other));
        if ty.content_eq(other) || self.takes_anything(ty) {
            return true;
        }
        if let Type::Union(union) = other {
            return union.types.iter().all(|part| self.type_takes(ty, part));
        }
        if let Type::Optional(optional) = other {
            return takes_nil(ty) && self.type_takes(ty, &optional.ty);
        }
        match ty {
            Type::Optional(optional) => {
                matches!(other, Type::Nil(_)) || self.type_takes(&optional.ty, other)
            }
            Type::Union(union) => union.types.iter().any(|part| self.type_takes(part, other)),
            Type::Function(function) => {
                matches!(other, Type::Function(_)) && self.takes_any_function(function)
            }
            Type::Name(_)
            | Type::Named(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Intersection(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Nil(_)
            | Type::True(_)
            | Type::False(_)
            | Type::StringLiteral(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_) => false,
        }
    }

    /// Whether `function` is a function type any function is, as
    /// `(A...) -> R...` is of the generic packs `A` and `R`.
    fn takes_any_function(&self, function: &FunctionType<'_>) -> bool {
        let [param] = &*function.params else {
            return false;
        };
        function.generics.is_none()
            && self.is_any_pack(&param.ty)
            && self.is_any_pack(&function.return_type)
    }

    /// Whether `ty` is a pack of any values: one of the function's generic
    /// packs, or `...any` or `...unknown`.
    fn is_any_pack(&self, ty: &Type<'_>) -> bool {
        if let Type::GenericPack(pack) = ty {
            return self.generic_packs.contains(&pack.name.text.as_str());
        }
        matches!(ty, Type::Variadic(variadic) if is_top(&variadic.ty))
    }
}

/// Whether `ty` is `any` or `unknown`.
fn is_top(ty: &Type<'_>) -> bool {
    let Type::Name(name) = unparenthesized(ty) else {
        return false;
    };
    matches!(name.text.as_str(), "any" | "unknown")
}

/// Which calls of `deprecated` one of `replacing`, the signatures of the
/// function its `use` names, takes in its place: a method of the deprecated
/// method's type when `is_method`.
pub(crate) fn replacement(
    deprecated: &Signature<'_>,
    replacing: &[Signature<'_>],
    is_method: bool,
) -> StdlibReplacement {
    let mut replacement = StdlibReplacement { is_method, accepted: 0, covered: 0, returning: 0 };
    for arg_count in 0..u64::BITS as usize {
        if !deprecated.takes(arg_count) {
            continue;
        }
        let bit = 1 << arg_count;
        replacement.accepted |= bit;
        let covering = replacing.iter().filter(|signature| {
            signature.takes(arg_count)
                && (0..arg_count).all(|index| {
                    deprecated.param(index).is_some_and(|param| signature.param_takes(index, param))
                })
        });
        for signature in covering {
            replacement.covered |= bit;
            if is_same_return(signature.returns, deprecated.returns) {
                replacement.returning |= bit;
            }
        }
    }
    replacement
}

fn is_same_return(returns: &[Type<'_>], other: &[Type<'_>]) -> bool {
    returns.len() == other.len()
        && returns
            .iter()
            .zip(other)
            .all(|(ty, other)| unparenthesized(ty).content_eq(unparenthesized(other)))
}

/// What a function returns, in order, as its return type says, which is
/// nothing when it has none.
fn returned<'ast>(ty: Option<&'ast Type<'ast>>) -> &'ast [Type<'ast>] {
    match ty.map(unparenthesized) {
        None => &[],
        Some(Type::Pack(pack)) => &pack.types,
        Some(ty) => slice::from_ref(ty),
    }
}

pub(crate) fn unparenthesized<'ty, 'ast>(mut ty: &'ty Type<'ast>) -> &'ty Type<'ast> {
    while let Type::Parenthesized(parenthesized) = ty {
        ty = &parenthesized.ty;
    }
    ty
}
