use luck_allocator::List;
use luck_ast::{
    Expression, FunctionAttribute, FunctionName, GetSpan as _, LocalAssignmentKind,
    LocalFunctionKind, Name, Statement, TypeDeclarationValue, TypedName,
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
            TokenKind::Local | TokenKind::Const => self.parse_local_stmt(attributes),
            TokenKind::Function => self.parse_function_decl(attributes),
            TokenKind::Export => self.parse_export_stmt(attributes),
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
        self.counts.name_uses += 1;
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
            self.counts.declarations += 1;
            self.expect_identifier_recover()
        });
        let end_span = match method {
            Some(method) => method.span,
            None => names.last().map_or(first.span, |name| name.span),
        };
        FunctionName::new(first.span.merge(end_span), names, method)
    }

    /// A statement that opens with `local` or `const`, the current token,
    /// which decides its kind.
    pub(super) fn parse_local_stmt(
        &mut self,
        attributes: List<'ast, FunctionAttribute<'ast>>,
    ) -> Statement<'ast> {
        let (function_kind, assignment_kind) = if self.is_at(TokenKind::Const) {
            (LocalFunctionKind::Const, LocalAssignmentKind::Const)
        } else {
            (LocalFunctionKind::Local, LocalAssignmentKind::Local)
        };
        let keyword = self.advance();
        if self.is_at(TokenKind::Function) {
            return self.parse_local_function(keyword, attributes, function_kind);
        }
        if let Some(attribute) = attributes.first() {
            self.error(attribute.span, "attributes are only allowed on function declarations");
        }
        self.parse_local_assignment(keyword, assignment_kind)
    }

    fn parse_local_function(
        &mut self,
        keyword: Span,
        attributes: List<'ast, FunctionAttribute<'ast>>,
        kind: LocalFunctionKind,
    ) -> Statement<'ast> {
        let start_span = attributes.first().map_or(keyword, |attribute| attribute.span);
        let function_span = self.advance();
        let name = self.expect_identifier_recover();
        self.counts.declarations += 1;
        let Some(body) = self.parse_function_body(function_span) else {
            return Statement::Error(start_span);
        };
        let span = start_span.merge(body.span);
        Statement::new_local_function(
            span,
            attributes,
            name,
            body,
            keyword.start,
            kind,
            self.allocator,
        )
    }

    fn parse_local_assignment(
        &mut self,
        local_span: Span,
        kind: LocalAssignmentKind,
    ) -> Statement<'ast> {
        let names = self.parse_local_names();
        let has_values = self.eat(TokenKind::Equal);
        let exprs = if has_values { self.parse_expr_list() } else { List::new() };

        let span = Span::new(local_span.start, self.previous_span.end);

        // Grammar: `const bindinglist '=' explist`, one value per name, except
        // that a last call or `...` may supply the rest.
        if kind.is_const()
            && !(has_values
                && (exprs.len() == names.len()
                    || exprs.last().is_some_and(Expression::is_multi_value)))
        {
            self.error(span, "missing initializer in `const` declaration");
        }

        Statement::new_local_assignment(span, names, exprs, kind, self.allocator)
    }

    fn parse_local_names(&mut self) -> List<'ast, TypedName<'ast>> {
        let start = self.scratch.typed_names.len();
        if let Some(first) = self.expect_identifier() {
            let first = self.parse_typed_name(first);
            self.scratch.typed_names.push(first);
            while self.eat(TokenKind::Comma) {
                let Some(name) = self.expect_identifier() else {
                    break;
                };
                let typed_name = self.parse_typed_name(name);
                self.scratch.typed_names.push(typed_name);
            }
        }
        List::split_off_in(&mut self.scratch.typed_names, start, self.allocator)
    }

    pub(super) fn parse_type_declaration(&mut self, export_span: Option<Span>) -> Statement<'ast> {
        let type_span = self.advance(); // `type`
        let start_span = export_span.unwrap_or(type_span);
        let is_exported = export_span.is_some();

        // `type function Name funcbody` takes no `=`, and the body is
        // ordinary Luau.
        if self.is_at(TokenKind::Function) {
            let function_span = self.advance();
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
            for local_name in &local.names {
                self.register_exported_name(local_name.name);
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
