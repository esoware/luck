use luck_allocator::List;

use crate::expr::Expression;
use crate::function::{FunctionAttribute, FunctionBody, Parameter, VarArgParam};
use crate::stmt::Block;
use crate::synth::Synth;
use crate::types::{GenericTypeList, Type};

/// `Default` is an empty, untyped `()`, so set only the fields you need.
#[derive(Debug, Default)]
pub struct FunctionSignature<'ast> {
    pub generics: Option<GenericTypeList<'ast>>,
    pub params: Vec<Parameter<'ast>>,
    /// Trailing `...`, optionally typed. See [`Synth::vararg_param`].
    pub vararg: Option<VarArgParam<'ast>>,
    pub return_type: Option<Type<'ast>>,
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the synthesizer is one type with its constructors split by node family"
)]
impl<'ast> Synth<'ast> {
    /// A Luau function attribute in the plain `@name` form.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn function_attribute(&self, name: &str) -> FunctionAttribute<'ast> {
        FunctionAttribute { span: self.next_span(), name: self.name(name), args: None }
    }

    /// A Luau function attribute in the bracketed `@[name(args)]` form.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn function_attribute_with_args<Args>(
        &self,
        name: &str,
        args: Args,
    ) -> FunctionAttribute<'ast>
    where
        Args: IntoIterator<Item = Expression<'ast>>,
        Args::IntoIter: ExactSizeIterator,
    {
        FunctionAttribute {
            span: self.next_span(),
            name: self.name(name),
            args: Some(self.list(args)),
        }
    }

    pub(super) fn function_body(
        &self,
        signature: FunctionSignature<'ast>,
        block: Block<'ast>,
    ) -> FunctionBody<'ast> {
        FunctionBody {
            span: self.next_span(),
            generics: signature.generics.map(|generics| self.boxed(generics)),
            params: List::from_iter_in(signature.params, self.allocator),
            vararg: signature.vararg.map(|vararg| self.boxed(vararg)),
            return_type: signature.return_type,
            block,
        }
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn param(&self, name: &str) -> Parameter<'ast> {
        Parameter { name: self.name(name), type_annotation: None }
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn param_typed(&self, name: &str, ty: Type<'ast>) -> Parameter<'ast> {
        Parameter { name: self.name(name), type_annotation: Some(ty) }
    }

    /// Lua 5.5's named `...name` form isn't supported, so `name` is `None`.
    #[must_use]
    pub fn vararg_param(&self, type_annotation: Option<Type<'ast>>) -> VarArgParam<'ast> {
        VarArgParam { span: self.next_span(), name: None, type_annotation }
    }

    pub(super) fn params(&self, names: &[&str]) -> Vec<Parameter<'ast>> {
        names.iter().map(|name| self.param(name)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::Fixture;
    use crate::stmt::Statement;

    #[test]
    fn builds_typed_function() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let signature = FunctionSignature {
            params: vec![synth.param_typed("n", synth.ty_named("number"))],
            return_type: Some(synth.ty_named("number")),
            ..FunctionSignature::default()
        };
        let return_stmt = synth.return_([synth.name_expr("n")]);
        let function = synth.function_def_full([], signature, synth.block([], Some(return_stmt)));

        let Expression::FunctionDef(function_def) = &function else {
            panic!("expected function def");
        };
        let [param] = &*function_def.body.params else {
            panic!("expected one parameter");
        };
        assert!(param.type_annotation.is_some());
        assert!(function_def.body.return_type.is_some());
    }

    #[test]
    fn builds_generic_function_with_attributes() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let signature = FunctionSignature {
            generics: Some(synth.generic_type_list([("T", false)])),
            params: vec![synth.param_typed("value", synth.ty_named("T"))],
            return_type: Some(synth.ty_named("T")),
            ..FunctionSignature::default()
        };
        let stmt = synth.local_function_full(
            "id",
            [synth.function_attribute("native")],
            signature,
            synth.block([], None),
        );

        let Statement::LocalFunction(local_function) = &stmt else {
            panic!("expected local function");
        };
        assert_eq!(local_function.attributes.len(), 1);
        assert!(local_function.body.generics.is_some());
    }

    #[test]
    fn builds_variadic_function() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let signature = FunctionSignature {
            params: vec![synth.param("first")],
            vararg: Some(synth.vararg_param(None)),
            ..FunctionSignature::default()
        };
        let function = synth.function_def_full([], signature, synth.block([], None));

        let Expression::FunctionDef(function_def) = &function else {
            panic!("expected function def");
        };
        assert_eq!(function_def.body.params.len(), 1);
        let vararg = function_def.body.vararg.as_ref().expect("vararg present");
        assert!(vararg.name.is_none());
        assert!(vararg.type_annotation.is_none());
    }

    #[test]
    fn builds_typed_vararg_param() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let vararg = synth.vararg_param(Some(synth.ty_named("number")));
        assert!(vararg.name.is_none());
        assert!(vararg.type_annotation.is_some());
    }

    #[test]
    fn builds_attribute_with_args() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let attribute = synth.function_attribute_with_args("deprecated", [synth.string("use y")]);
        assert_eq!(attribute.args.as_ref().map(|args| args.len()), Some(1));
        let empty = synth.function_attribute_with_args("deprecated", []);
        assert_eq!(empty.args.as_ref().map(|args| args.len()), Some(0));
        assert!(synth.function_attribute("native").args.is_none());
    }
}
