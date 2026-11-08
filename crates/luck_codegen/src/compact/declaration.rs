use luck_ast::{
    AttributedName, FunctionDecl, FunctionName, GlobalDeclaration, GlobalFunction, GlobalStar,
    LocalAssignment, LocalFunction, TypeDeclaration, TypeDeclarationValue,
};

use super::CompactPrinter;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the printer is one type with its emitters split by node family"
)]
impl CompactPrinter {
    pub(super) fn emit_function_declaration(&mut self, function_declaration: &FunctionDecl) {
        self.emit_function_attributes(&function_declaration.attributes);
        self.emit_fixed("function");
        self.emit_function_name(&function_declaration.name);
        self.emit_function_body(&function_declaration.body);
    }

    fn emit_function_name(&mut self, name: &FunctionName) {
        self.emit_joined(&name.names, ".", Self::emit_token);
        if let Some(method_name) = &name.method {
            self.emit_fixed(":");
            self.emit_token(method_name);
        }
    }

    pub(super) fn emit_local_function(&mut self, local_function: &LocalFunction) {
        self.emit_function_attributes(&local_function.attributes);
        if local_function.is_exported {
            self.emit_fixed("export");
        } else {
            self.emit_fixed(if local_function.is_const { "const" } else { "local" });
        }
        self.emit_fixed("function");
        self.emit_token(&local_function.name);
        self.emit_function_body(&local_function.body);
    }

    pub(super) fn emit_local_assignment(&mut self, local_assignment: &LocalAssignment) {
        if local_assignment.is_exported {
            self.emit_fixed("export");
        }
        self.emit_fixed(if local_assignment.is_const { "const" } else { "local" });
        self.emit_punctuated(&local_assignment.names, ",", Self::emit_attributed_name);
        if let Some(exprs) = &local_assignment.exprs {
            self.emit_fixed("=");
            self.emit_punctuated(exprs, ",", Self::emit_expression);
        }
    }

    fn emit_attributed_name(&mut self, attributed: &AttributedName) {
        self.emit_token(&attributed.name);
        if let Some(ty) = &attributed.type_annotation {
            self.emit_fixed(":");
            self.emit_type(ty);
        }
        if let Some(attribute) = &attributed.attribute {
            self.emit_fixed("<");
            self.emit_token(&attribute.name);
            self.emit_fixed(">");
        }
    }

    pub(super) fn emit_type_declaration(&mut self, type_declaration: &TypeDeclaration) {
        if type_declaration.is_exported {
            self.emit_fixed("export");
        }
        self.emit_fixed("type");
        // Luau `type function Name funcbody` - no `=`.
        if matches!(type_declaration.type_value, TypeDeclarationValue::TypeFunction(_)) {
            self.emit_fixed("function");
        }
        self.emit_token(&type_declaration.name);
        if let Some(generics) = &type_declaration.generics {
            self.emit_generic_type_list(generics);
        }
        match &type_declaration.type_value {
            TypeDeclarationValue::Alias(ty) => {
                self.emit_fixed("=");
                self.emit_type(ty);
            }
            TypeDeclarationValue::TypeFunction(body) => self.emit_function_body(body),
        }
    }

    pub(super) fn emit_global_declaration(&mut self, global_declaration: &GlobalDeclaration) {
        self.emit_fixed("global");
        self.emit_punctuated(&global_declaration.names, ",", Self::emit_attributed_name);
        if let Some(exprs) = &global_declaration.exprs {
            self.emit_fixed("=");
            self.emit_punctuated(exprs, ",", Self::emit_expression);
        }
    }

    pub(super) fn emit_global_function(&mut self, global_function: &GlobalFunction) {
        self.emit_fixed("global");
        self.emit_fixed("function");
        self.emit_token(&global_function.name);
        self.emit_function_body(&global_function.body);
    }

    pub(super) fn emit_global_star(&mut self, global_star: &GlobalStar) {
        self.emit_fixed("global");
        if let Some(attribute) = &global_star.attribute {
            self.emit_fixed("<");
            self.emit_token(&attribute.name);
            self.emit_fixed(">");
        }
        self.emit_fixed("*");
    }
}
