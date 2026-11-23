use luck_allocator::List;
use luck_ast::{
    Attribute, AttributedName, FunctionAttribute, FunctionName, GetSpan as _, LocalAssignmentKind,
    LocalFunctionKind, Name, Statement, TypeDeclarationValue, is_multi_value,
};
use luck_diagnostics::Diagnostic;
use luck_token::{Span, TokenKind};

use super::Parser;
use super::types::GenericsOwner;

impl<'ast> Parser<'ast> {
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_attributed_function(&mut self) -> Statement<'ast> {
        let attributes = self.parse_function_attributes();
        match self.peek() {
            TokenKind::Local => self.parse_local_stmt(attributes),
            TokenKind::Function => self.parse_function_decl(attributes),
            TokenKind::Export => self.parse_export_stmt(attributes),
            TokenKind::Const => self.parse_const_stmt(attributes),
            _ => {
                let span = self.token.span;
                self.syntax_error(
                    span,
                    "expected `function`, `local`, `const`, or `export` after attribute",
                );
                Statement::Error(span)
            }
        }
    }

    pub(super) fn parse_function_decl(
        &mut self,
        attributes: List<'ast, FunctionAttribute<'ast>>,
    ) -> Statement<'ast> {
        let function_span = self.advance();
        let name = self.parse_function_name();
        let start_span = attributes.first().map_or(function_span, |attribute| attribute.span);
        let Some(body) = self.parse_function_body(function_span) else {
            return Statement::Error(start_span);
        };
        let span = start_span.merge(body.span);
        Statement::new_function_decl(span, attributes, name, body, self.allocator)
    }

    fn parse_function_name(&mut self) -> FunctionName<'ast> {
        let first = self.expect_identifier_recover();
        self.counts.name_uses = self.counts.name_uses.wrapping_add(1);
        let names = if self.is_at(TokenKind::Dot) {
            let start = self.scratch.names.len();
            self.scratch.names.push(first);
            while self.eat(TokenKind::Dot) {
                let name = self.expect_identifier_recover();
                self.scratch.names.push(name);
            }
            List::split_off_in(&mut self.scratch.names, start, self.allocator)
        } else {
            List::from_array_in([first], self.allocator)
        };
        // A method declares `self`.
        let method = self.eat(TokenKind::Colon).then(|| {
            self.counts.declarations = self.counts.declarations.wrapping_add(1);
            self.expect_identifier_recover()
        });
        let end_span = method
            .map_or_else(|| names.last().map_or(first.span, |name| name.span), |name| name.span);
        FunctionName::new(first.span.merge(end_span), names, method)
    }

    pub(super) fn parse_local_stmt(
        &mut self,
        attributes: List<'ast, FunctionAttribute<'ast>>,
    ) -> Statement<'ast> {
        let local_span = self.advance();
        if self.is_at(TokenKind::Function) {
            return self.parse_local_function(local_span, attributes, LocalFunctionKind::Local);
        }
        if let Some(attribute) = attributes.first() {
            self.error(attribute.span, "attributes are only allowed on function declarations");
        }
        self.parse_local_assignment(local_span, LocalAssignmentKind::Local)
    }

    pub(super) fn parse_const_stmt(
        &mut self,
        attributes: List<'ast, FunctionAttribute<'ast>>,
    ) -> Statement<'ast> {
        let const_span = self.advance(); // `const`
        if self.is_at(TokenKind::Function) {
            return self.parse_local_function(const_span, attributes, LocalFunctionKind::Const);
        }
        if let Some(attribute) = attributes.first() {
            self.error(attribute.span, "attributes are only allowed on function declarations");
        }
        self.parse_local_assignment(const_span, LocalAssignmentKind::Const)
    }

    fn parse_local_function(
        &mut self,
        local_span: Span,
        attributes: List<'ast, FunctionAttribute<'ast>>,
        kind: LocalFunctionKind,
    ) -> Statement<'ast> {
        let start_span = attributes.first().map_or(local_span, |attribute| attribute.span);
        let function_span = self.advance();
        let name = self.expect_identifier_recover();
        self.counts.declarations = self.counts.declarations.wrapping_add(1);
        let Some(body) = self.parse_function_body(function_span) else {
            return Statement::Error(start_span);
        };
        let span = start_span.merge(body.span);
        Statement::new_local_function(span, attributes, name, body, kind, self.allocator)
    }

    fn parse_local_assignment(
        &mut self,
        local_span: Span,
        kind: LocalAssignmentKind,
    ) -> Statement<'ast> {
        let names = self.parse_attname_list();

        // 5.4 manual section 3.3.7: "A list of variables can contain at most
        // one to-be-closed variable."
        let mut has_close = false;
        for attribute in names.iter().filter_map(|attributed| attributed.attribute.as_deref()) {
            if attribute.name.text == "close" {
                if has_close {
                    self.error(attribute.span, "multiple to-be-closed variables in local list");
                }
                has_close = true;
            }
        }

        let has_values = self.eat(TokenKind::Equal);
        let exprs = if has_values { self.parse_expr_list() } else { List::new() };

        let span = Span::new(local_span.start, self.previous_span.end);

        // Grammar: `const bindinglist '=' explist`, one value per name, except
        // that a last call or `...` may supply the rest.
        if kind.is_const()
            && !(has_values
                && (exprs.len() == names.len() || exprs.last().is_some_and(is_multi_value)))
        {
            self.error(span, "missing initializer in `const` declaration");
        }

        Statement::new_local_assignment(span, names, exprs, kind, self.allocator)
    }

    /// The leading attribute is new in 5.5; in 5.4, attributes only follow
    /// names.
    fn parse_attname_list(&mut self) -> List<'ast, AttributedName<'ast>> {
        // Lua 5.5: optional leading attribute before the first name.
        let leading_attribute = (self.version.has_leading_attributes()
            && self.is_at(TokenKind::Less))
        .then(|| self.parse_attribute());
        self.parse_attributed_names(leading_attribute)
    }

    /// `global <attrib> name...` also enters here directly.
    fn parse_attributed_names(
        &mut self,
        leading_attribute: Option<Attribute<'ast>>,
    ) -> List<'ast, AttributedName<'ast>> {
        let start = self.scratch.attributed_names.len();
        if let Some(first) = self.parse_attributed_name(leading_attribute) {
            self.scratch.attributed_names.push(first);
            while self.eat(TokenKind::Comma) {
                let Some(name) = self.parse_attributed_name(leading_attribute) else {
                    break;
                };
                self.scratch.attributed_names.push(name);
            }
        }
        List::split_off_in(&mut self.scratch.attributed_names, start, self.allocator)
    }

    /// Parse `Name [: T] [attrib]`, or `None` after recording the error when
    /// the name is missing.
    ///
    /// 5.5 manual section 3.3.7: a prefixed attribute applies to every name in
    /// the list, so each name gets a copy (emitting it after every name means
    /// the same thing). A name's own postfixed attribute replaces the copy.
    fn parse_attributed_name(
        &mut self,
        leading_attribute: Option<Attribute<'ast>>,
    ) -> Option<AttributedName<'ast>> {
        let name = self.expect_identifier()?;
        self.counts.declarations = self.counts.declarations.wrapping_add(1);
        // Luau: type annotation after the name.
        let type_annotation = self.try_parse_type_annotation();
        let attribute =
            self.try_parse_attribute().or(leading_attribute).map(|attribute| self.alloc(attribute));
        Some(AttributedName::new(name, type_annotation, attribute))
    }

    pub(super) fn parse_global_stmt(&mut self) -> Statement<'ast> {
        let global_span = self.advance();

        // `global function name(...) ... end`
        if self.is_at(TokenKind::Function) {
            let function_span = self.advance();
            let name = self.expect_identifier_recover();
            let Some(body) = self.parse_function_body(function_span) else {
                return Statement::Error(global_span);
            };
            let span = global_span.merge(body.span);
            return Statement::new_global_function(span, name, body, self.allocator);
        }

        // `global *`
        if self.is_at(TokenKind::Star) {
            let star = self.advance();
            let span = global_span.merge(star);
            return Statement::new_global_star(span, None, self.allocator);
        }

        // `global <attrib> *` puts the attribute before the star. Otherwise
        // the attribute leads an attnamelist and the shared parser takes it.
        if self.is_at(TokenKind::Less) {
            let attribute = self.parse_attribute();
            self.reject_global_close(attribute);
            if self.is_at(TokenKind::Star) {
                let star = self.advance();
                let span = global_span.merge(star);
                return Statement::new_global_star(span, Some(attribute), self.allocator);
            }
            let names = self.parse_attributed_names(Some(attribute));
            return self.finish_global_declaration(global_span, names, Some(attribute));
        }

        // `global name [attrib] {, name [attrib]} ['=' explist]`
        let names = self.parse_attname_list();
        self.finish_global_declaration(global_span, names, None)
    }

    /// Parse the optional `= explist` tail of a `global` declaration
    /// (5.5 manual section 3.3.7: `stat ::= global attnamelist ['=' explist]`).
    /// `leading_attribute` has already been checked, so its copies are not.
    fn finish_global_declaration(
        &mut self,
        global_span: Span,
        names: List<'ast, AttributedName<'ast>>,
        leading_attribute: Option<Attribute<'ast>>,
    ) -> Statement<'ast> {
        for attribute in
            names.iter().filter_map(|attributed| attributed.attribute.as_deref()).filter(
                |attribute| leading_attribute.is_none_or(|leading| leading.span != attribute.span),
            )
        {
            self.reject_global_close(*attribute);
        }

        let exprs = if self.eat(TokenKind::Equal) { self.parse_expr_list() } else { List::new() };
        let span = Span::new(global_span.start, self.previous_span.end);
        Statement::new_global_declaration(span, names, exprs, self.allocator)
    }

    /// 5.5 manual section 3.3.7: only local variables can have the close
    /// attribute.
    fn reject_global_close(&mut self, attribute: Attribute<'ast>) {
        if attribute.name.text == "close" {
            self.error(attribute.span, "only local variables can have the `close` attribute");
        }
    }

    pub(super) fn parse_type_declaration(&mut self, export_span: Option<Span>) -> Statement<'ast> {
        let type_span = self.advance(); // `type`
        let start_span = export_span.unwrap_or(type_span);
        let is_exported = export_span.is_some();

        // `type function Name funcbody` takes no `=`, and the body is
        // ordinary Luau.
        if self.is_at(TokenKind::Function) {
            let function_span = self.advance();
            self.has_type_function = true;
            let name = self.expect_identifier_recover();
            let Some(body) = self.parse_function_body(function_span) else {
                return Statement::Error(start_span);
            };
            let span = start_span.merge(body.span);
            let value = TypeDeclarationValue::TypeFunction(self.alloc(body));
            return Statement::new_type_declaration(
                span,
                is_exported,
                name,
                None,
                value,
                self.allocator,
            );
        }

        let name = self.expect_identifier_recover();
        let generics = self.is_at(TokenKind::Less).then(|| {
            let generics = self.parse_generic_type_list(GenericsOwner::TypeAlias);
            self.alloc(generics)
        });
        self.expect(TokenKind::Equal);
        let ty = self.parse_type();
        let span = start_span.merge(ty.span());
        Statement::new_type_declaration(
            span,
            is_exported,
            name,
            generics,
            TypeDeclarationValue::Alias(ty),
            self.allocator,
        )
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_export_stmt(
        &mut self,
        attributes: List<'ast, FunctionAttribute<'ast>>,
    ) -> Statement<'ast> {
        let export_span = self.advance(); // `export`

        if self.is_at(TokenKind::Type) {
            if !attributes.is_empty() {
                self.error(export_span, "attributes are only allowed on exported functions");
            }
            return self.parse_type_declaration(Some(export_span));
        }

        if !self.version.has_value_exports() {
            self.error(export_span, "value exports are not supported in this Lua version");
            return Statement::Error(export_span);
        }

        if self.block_depth != 1 {
            self.error(export_span, "value exports are only allowed at module scope");
        }
        if self.has_module_return {
            self.error(
                export_span,
                "a module cannot use both value exports and `return` statements",
            );
        }
        self.has_value_exports = true;

        let stmt = match self.peek() {
            TokenKind::Local => {
                let local_span = self.advance();
                if self.is_at(TokenKind::Function) {
                    self.error(
                        export_span,
                        "`export local function` is not supported, use `export function`",
                    );
                    self.parse_local_function(local_span, attributes, LocalFunctionKind::Local)
                } else {
                    if !attributes.is_empty() {
                        self.error(
                            export_span,
                            "attributes are only allowed on exported functions",
                        );
                    }
                    self.parse_local_assignment(export_span, LocalAssignmentKind::ExportLocal)
                }
            }
            TokenKind::Function => {
                self.parse_local_function(export_span, attributes, LocalFunctionKind::Export)
            }
            TokenKind::Const => {
                self.advance();
                if !attributes.is_empty() {
                    self.error(export_span, "attributes are only allowed on exported functions");
                }
                if self.is_at(TokenKind::Function) {
                    self.error(
                        export_span,
                        "`export const function` is not supported, use `export function`",
                    );
                    self.parse_local_function(export_span, List::new(), LocalFunctionKind::Const)
                } else {
                    self.parse_local_assignment(export_span, LocalAssignmentKind::ExportConst)
                }
            }
            _ => {
                let span = self.token.span;
                self.syntax_error(
                    span,
                    "`export` must be followed by `type`, `local`, `const`, or `function`",
                );
                return Statement::Error(export_span.merge(span));
            }
        };

        self.register_exported_names(&stmt);
        stmt
    }

    fn register_exported_names(&mut self, stmt: &Statement<'ast>) {
        if let Statement::LocalAssignment(local) = stmt
            && local.kind.is_exported()
        {
            for attributed in &local.names {
                self.register_exported_name(attributed.name);
            }
        }
        if let Statement::LocalFunction(function) = stmt
            && function.kind.is_exported()
        {
            self.register_exported_name(function.name);
        }
    }

    fn register_exported_name(&mut self, name: Name<'ast>) {
        if let Some(&first) = self.exported_names.get(name.text.as_str()) {
            let message = format!("duplicate exported identifier `{}`", name.text);
            self.errors.push(
                Diagnostic::e008(name.span, message).with_label(first, "first exported here"),
            );
        } else {
            self.exported_names.insert(name.text.as_str(), name.span);
        }
    }
}
