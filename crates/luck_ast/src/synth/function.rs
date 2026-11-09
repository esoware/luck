use crate::expr::Expression;
use crate::function::{FunctionAttribute, FunctionBody, Parameter, VarArgParam};
use crate::punctuated::Punctuated;
use crate::stmt::Block;
use crate::synth::Synth;
use crate::types::{GenericTypeList, Type};

/// `Default` is an empty, untyped `()`, so set only the fields you need.
#[derive(Debug, Default)]
pub struct FunctionSignature {
    pub generics: Option<GenericTypeList>,
    pub params: Vec<Parameter>,
    /// Trailing `...`, optionally typed. See [`Synth::vararg_param`].
    pub vararg: Option<VarArgParam>,
    pub return_type: Option<Type>,
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the synthesizer is one type with its constructors split by node family"
)]
impl Synth {
    /// A Luau function attribute: `@name` when `args` is `None`, the
    /// bracketed `@[name(args)]` form otherwise.
    #[must_use]
    pub fn function_attribute(
        &self,
        name: &str,
        args: Option<Vec<Expression>>,
    ) -> FunctionAttribute {
        FunctionAttribute {
            span: self.next_span(),
            name: self.identifier(name),
            args: args.map(Punctuated::from_items),
        }
    }

    pub(super) fn function_body(&self, signature: FunctionSignature, block: Block) -> FunctionBody {
        FunctionBody {
            span: self.next_span(),
            generics: signature.generics.map(Box::new),
            params: Punctuated::from_items(signature.params),
            vararg: signature.vararg.map(Box::new),
            return_type: signature.return_type,
            block,
        }
    }

    #[must_use]
    pub fn param(&self, name: &str) -> Parameter {
        Parameter { span: self.next_span(), name: self.identifier(name), type_annotation: None }
    }

    #[must_use]
    pub fn param_typed(&self, name: &str, ty: Type) -> Parameter {
        Parameter { span: self.next_span(), name: self.identifier(name), type_annotation: Some(ty) }
    }

    /// Lua 5.5's named `...name` form isn't supported, so `name` is `None`.
    #[must_use]
    pub fn vararg_param(&self, type_annotation: Option<Type>) -> VarArgParam {
        VarArgParam { span: self.next_span(), name: None, type_annotation }
    }

    pub(super) fn params(&self, names: &[&str]) -> Vec<Parameter> {
        names.iter().map(|name| self.param(name)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stmt::Statement;

    #[test]
    fn builds_typed_function() {
        let synth = Synth::new();
        let signature = FunctionSignature {
            params: vec![synth.param_typed("n", synth.ty_named("number"))],
            return_type: Some(synth.ty_named("number")),
            ..FunctionSignature::default()
        };
        let return_stmt = synth.return_(vec![synth.name_expr("n")]);
        let function =
            synth.function_def_full(Vec::new(), signature, synth.block(vec![], Some(return_stmt)));

        let Expression::FunctionDef(function_def) = &function else {
            panic!("expected function def");
        };
        assert_eq!(function_def.body.params.len(), 1);
        assert!(function_def.body.params.first().unwrap().type_annotation.is_some());
        assert!(function_def.body.return_type.is_some());
    }

    #[test]
    fn builds_generic_function_with_attributes() {
        let synth = Synth::new();
        let signature = FunctionSignature {
            generics: Some(synth.generic_type_list(vec![("T", false)])),
            params: vec![synth.param_typed("value", synth.ty_named("T"))],
            return_type: Some(synth.ty_named("T")),
            ..FunctionSignature::default()
        };
        let stmt = synth.local_function_full(
            "id",
            vec![synth.function_attribute("native", None)],
            signature,
            synth.block(vec![], None),
        );

        let Statement::LocalFunction(local_function) = &stmt else {
            panic!("expected local function");
        };
        assert_eq!(local_function.attributes.len(), 1);
        assert!(local_function.body.generics.is_some());
    }

    #[test]
    fn builds_variadic_function() {
        let synth = Synth::new();
        let signature = FunctionSignature {
            params: vec![synth.param("first")],
            vararg: Some(synth.vararg_param(None)),
            ..FunctionSignature::default()
        };
        let function = synth.function_def_full(Vec::new(), signature, synth.block(vec![], None));

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
        let synth = Synth::new();
        let vararg = synth.vararg_param(Some(synth.ty_named("number")));
        assert!(vararg.name.is_none());
        assert!(vararg.type_annotation.is_some());
    }

    #[test]
    fn builds_attribute_with_args() {
        let synth = Synth::new();
        let attribute = synth.function_attribute("deprecated", Some(vec![synth.string("use y")]));
        let args = attribute.args.as_ref().expect("args present");
        assert_eq!(args.len(), 1);

        let plain = synth.function_attribute("native", None);
        assert!(plain.args.is_none());
    }
}
