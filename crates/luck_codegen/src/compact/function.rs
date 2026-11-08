use luck_ast::{FunctionAttribute, FunctionBody, Parameter};

use super::CompactPrinter;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the printer is one type with its emitters split by node family"
)]
impl CompactPrinter {
    /// Emit the Luau attribute list, which must survive because some
    /// attributes change runtime behavior (`@native`). Arguments only exist
    /// on the bracketed form, so attributes with args re-emit as
    /// `@[name(...)]`.
    pub(super) fn emit_function_attributes(&mut self, attributes: &[FunctionAttribute]) {
        for attribute in attributes {
            self.emit_fixed("@");
            if let Some(args) = &attribute.args {
                self.emit_fixed("[");
                self.emit_token(&attribute.name);
                self.emit_fixed("(");
                self.emit_punctuated(args, ",", Self::emit_expression);
                self.emit_fixed(")");
                self.emit_fixed("]");
            } else {
                self.emit_token(&attribute.name);
            }
        }
    }

    pub(super) fn emit_function_body(&mut self, body: &FunctionBody) {
        // Luau: `<T, U...>` generics sit between the function name and `(`.
        if let Some(generics) = &body.generics {
            self.emit_generic_type_list(generics);
        }
        self.emit_fixed("(");
        self.emit_joined(&body.params.items, ",", Self::emit_param);
        if let Some(vararg) = &body.vararg {
            // The AST doesn't store the comma between the last param and `...`.
            if !body.params.is_empty() {
                self.emit_fixed(",");
            }
            self.emit_fixed("...");
            if let Some(name) = &vararg.name {
                self.emit_token(name);
            }
            if let Some(ty) = &vararg.type_annotation {
                self.emit_fixed(":");
                self.emit_type(ty);
            }
        }
        self.emit_fixed(")");
        if let Some(return_type) = &body.return_type {
            self.emit_fixed(":");
            self.emit_type(return_type);
        }
        self.emit_block(&body.block);
        self.emit_fixed("end");
    }

    pub(super) fn emit_param(&mut self, param: &Parameter) {
        self.emit_token(&param.name);
        if let Some(ty) = &param.type_annotation {
            self.emit_fixed(":");
            self.emit_type(ty);
        }
    }
}
