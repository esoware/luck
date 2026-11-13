use luck_ast::{
    Attribute, AttributedName, Expression, FunctionAttribute, FunctionDecl, FunctionName,
    GlobalDeclaration, GlobalFunction, GlobalStar, LocalAssignment, LocalFunction, Punctuated,
    Statement, TypeDeclaration, TypeDeclarationValue, is_multi_value,
};
use luck_token::{Span, Token, TokenKind};

use super::Parser;
use super::types::GenericsOwner;

/// The declaration that introduced a local binding, which fixes its
/// `is_const` and `is_exported` flags.
#[derive(Clone, Copy)]
enum LocalKind {
    Local,
    Const,
    ExportedLocal,
    /// Luau `export const`, and `export function`, whose binding is read-only
    /// too.
    ExportedConst,
}

impl LocalKind {
    const fn is_const(self) -> bool {
        match self {
            Self::Const | Self::ExportedConst => true,
            Self::Local | Self::ExportedLocal => false,
        }
    }

    const fn is_exported(self) -> bool {
        match self {
            Self::ExportedLocal | Self::ExportedConst => true,
            Self::Local | Self::Const => false,
        }
    }
}

/// The span of a `local` or `global` declaration, from `start_span` through
/// its last value, or through its last name and that name's annotation and
/// attribute when it has no values.
fn declaration_span(
    start_span: Span,
    names: &Punctuated<AttributedName>,
    exprs: Option<&Punctuated<Expression>>,
) -> Span {
    let end_span = exprs
        .and_then(Punctuated::last)
        .map(Expression::span)
        .or_else(|| {
            names.last().map(|attributed| {
                let mut span = attributed.name.span;
                if let Some(annotation) = &attributed.type_annotation {
                    span = span.merge(annotation.span());
                }
                if let Some(attribute) = &attributed.attribute {
                    span = span.merge(attribute.span);
                }
                span
            })
        })
        .unwrap_or(start_span);
    start_span.merge(end_span)
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl Parser<'_> {
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_attributed_function(&mut self) -> Statement {
        let attributes = self.parse_function_attributes();
        match self.peek() {
            TokenKind::Local => self.parse_local_stmt(attributes),
            TokenKind::Function => self.parse_function_decl(attributes),
            TokenKind::Identifier(name) if name == "export" => self.parse_export_stmt(attributes),
            TokenKind::Identifier(name) if name == "const" => self.parse_const_stmt(attributes),
            _ => {
                let span = self.current_span();
                self.error(
                    span,
                    "expected `function`, `local`, `const`, or `export` after attribute",
                );
                Statement::Error(span)
            }
        }
    }

    pub(super) fn parse_function_decl(&mut self, attributes: Vec<FunctionAttribute>) -> Statement {
        let function_span = self.advance_span();
        let name = self.parse_function_name();
        let body = self.parse_function_body();
        let start_span = attributes.first().map_or(function_span, |attribute| attribute.span);
        let span = start_span.merge(body.span);

        Statement::FunctionDecl(Box::new(FunctionDecl { span, attributes, name, body }))
    }

    fn parse_function_name(&mut self) -> FunctionName {
        let first = self.expect_identifier_recover();
        let start_span = first.span;
        let mut names = vec![first];

        while self.eat(&TokenKind::Dot) {
            let name = self.expect_identifier_recover();
            names.push(name);
        }

        let method = self.eat(&TokenKind::Colon).then(|| self.expect_identifier_recover());

        let end_span = method
            .as_ref()
            .map(|name| name.span)
            .or_else(|| names.last().map(|name| name.span))
            .unwrap_or(start_span);

        FunctionName { span: start_span.merge(end_span), names, method }
    }

    pub(super) fn parse_local_stmt(&mut self, attributes: Vec<FunctionAttribute>) -> Statement {
        let local_span = self.advance_span();

        if matches!(self.peek(), TokenKind::Function) {
            return self.parse_local_function(local_span, attributes, LocalKind::Local);
        }

        if let Some(attribute) = attributes.first() {
            self.error(attribute.span, "attributes are only allowed on function declarations");
        }
        self.parse_local_assignment(local_span, LocalKind::Local)
    }

    pub(super) fn parse_const_stmt(&mut self, attributes: Vec<FunctionAttribute>) -> Statement {
        let const_span = self.advance_span(); // contextual `const`
        if matches!(self.peek(), TokenKind::Function) {
            return self.parse_local_function(const_span, attributes, LocalKind::Const);
        }
        if let Some(attribute) = attributes.first() {
            self.error(attribute.span, "attributes are only allowed on function declarations");
        }
        self.parse_local_assignment(const_span, LocalKind::Const)
    }

    fn parse_local_function(
        &mut self,
        local_span: Span,
        attributes: Vec<FunctionAttribute>,
        kind: LocalKind,
    ) -> Statement {
        self.advance_span(); // `function`
        let name = self.expect_identifier_recover();
        let body = self.parse_function_body();
        let start_span = attributes.first().map_or(local_span, |attribute| attribute.span);
        let span = start_span.merge(body.span);

        Statement::LocalFunction(Box::new(LocalFunction {
            span,
            attributes,
            name,
            body,
            is_const: kind.is_const(),
            is_exported: kind.is_exported(),
        }))
    }

    fn parse_local_assignment(&mut self, local_span: Span, kind: LocalKind) -> Statement {
        let names = self.parse_attname_list();

        // 5.4 manual section 3.3.7: "A list of variables can contain at most
        // one to-be-closed variable."
        let mut has_close = false;
        for attribute in names.iter().filter_map(|attributed| attributed.attribute.as_deref()) {
            if attribute.name.identifier() == Some("close") {
                if has_close {
                    self.error(attribute.span, "multiple to-be-closed variables in local list");
                }
                has_close = true;
            }
        }

        let exprs = self.eat(&TokenKind::Equal).then(|| self.parse_expr_list());

        let span = declaration_span(local_span, &names, exprs.as_ref());

        // Grammar: `const bindinglist '=' explist`, one value per name, except
        // that a last call or `...` may supply the rest.
        if kind.is_const()
            && !exprs.as_ref().is_some_and(|values| {
                values.len() == names.len() || values.last().is_some_and(is_multi_value)
            })
        {
            self.error(span, "missing initializer in `const` declaration");
        }

        Statement::LocalAssignment(Box::new(LocalAssignment {
            span,
            names,
            exprs,
            is_const: kind.is_const(),
            is_exported: kind.is_exported(),
        }))
    }

    /// The leading attribute is new in 5.5; in 5.4, attributes only follow
    /// names.
    fn parse_attname_list(&mut self) -> Punctuated<AttributedName> {
        // Lua 5.5: optional leading attribute before the first name.
        let leading_attribute = (self.version.has_leading_attributes()
            && matches!(self.peek(), TokenKind::Less))
        .then(|| self.parse_attribute());
        self.parse_attributed_names(leading_attribute.as_ref())
    }

    /// `global <attrib> name...` also enters here directly.
    fn parse_attributed_names(
        &mut self,
        leading_attribute: Option<&Attribute>,
    ) -> Punctuated<AttributedName> {
        let Some(first) = self.parse_attributed_name(leading_attribute) else {
            return Punctuated::empty();
        };
        let mut names = vec![first];

        while self.eat(&TokenKind::Comma) {
            let Some(name) = self.parse_attributed_name(leading_attribute) else {
                break;
            };
            names.push(name);
        }

        Punctuated::from_items(names)
    }

    /// Parse `Name [: T] [attrib]`, or `None` after recording the error when
    /// the name is missing.
    ///
    /// 5.5 manual section 3.3.7: a prefixed attribute applies to every name in
    /// the list, so each name gets a copy (emitting it after every name means
    /// the same thing). A name's own postfixed attribute replaces the copy.
    fn parse_attributed_name(
        &mut self,
        leading_attribute: Option<&Attribute>,
    ) -> Option<AttributedName> {
        let name = self.expect_identifier()?;
        // Luau: type annotation after the name.
        let type_annotation = self.try_parse_type_annotation();
        let attribute = self.try_parse_attribute().or_else(|| leading_attribute.cloned());
        Some(AttributedName { name, type_annotation, attribute: attribute.map(Box::new) })
    }

    pub(super) fn parse_global_stmt(&mut self) -> Statement {
        let global_span = self.advance_span();

        // `global function name(...) ... end`
        if self.eat(&TokenKind::Function) {
            let name = self.expect_identifier_recover();
            let body = self.parse_function_body();
            let span = global_span.merge(body.span);
            return Statement::GlobalFunction(Box::new(GlobalFunction { span, name, body }));
        }

        // `global *`
        if matches!(self.peek(), TokenKind::Star) {
            let star = self.advance_span();
            let span = global_span.merge(star);
            return Statement::GlobalStar(Box::new(GlobalStar { span, attribute: None }));
        }

        // `global <attrib> *` puts the attribute before the star. Otherwise
        // the attribute leads an attnamelist and the shared parser takes it.
        if matches!(self.peek(), TokenKind::Less) {
            let attribute = self.parse_attribute();
            self.reject_global_close(&attribute);
            if matches!(self.peek(), TokenKind::Star) {
                let star = self.advance_span();
                let span = global_span.merge(star);
                return Statement::GlobalStar(Box::new(GlobalStar {
                    span,
                    attribute: Some(attribute),
                }));
            }
            let names = self.parse_attributed_names(Some(&attribute));
            return self.finish_global_declaration(global_span, names, Some(&attribute));
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
        names: Punctuated<AttributedName>,
        leading_attribute: Option<&Attribute>,
    ) -> Statement {
        for attribute in
            names.iter().filter_map(|attributed| attributed.attribute.as_deref()).filter(
                |attribute| leading_attribute.is_none_or(|leading| leading.span != attribute.span),
            )
        {
            self.reject_global_close(attribute);
        }

        let exprs = self.eat(&TokenKind::Equal).then(|| self.parse_expr_list());

        let span = declaration_span(global_span, &names, exprs.as_ref());

        Statement::GlobalDeclaration(Box::new(GlobalDeclaration { span, names, exprs }))
    }

    /// 5.5 manual section 3.3.7: only local variables can have the close
    /// attribute.
    fn reject_global_close(&mut self, attribute: &Attribute) {
        if attribute.name.identifier() == Some("close") {
            self.error(attribute.span, "only local variables can have the `close` attribute");
        }
    }

    pub(super) fn parse_type_declaration(&mut self, export_span: Option<Span>) -> Statement {
        let type_span = self.advance_span(); // `type` identifier
        let start_span = export_span.unwrap_or(type_span);

        // `type function Name funcbody` takes no `=`, and the body is
        // ordinary Luau.
        if self.eat(&TokenKind::Function) {
            self.has_type_function = true;
            let name = self.expect_identifier_recover();
            let body = self.parse_function_body();
            let span = start_span.merge(body.span);
            return Statement::TypeDeclaration(Box::new(TypeDeclaration {
                span,
                is_exported: export_span.is_some(),
                name,
                generics: None,
                type_value: TypeDeclarationValue::TypeFunction(Box::new(body)),
            }));
        }

        let name = self.expect_identifier_recover();

        let generics = matches!(self.peek(), TokenKind::Less)
            .then(|| Box::new(self.parse_generic_type_list(GenericsOwner::TypeAlias)));

        self.expect(&TokenKind::Equal);

        let alias_type = self.parse_type();
        let span = start_span.merge(alias_type.span());

        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span,
            is_exported: export_span.is_some(),
            name,
            generics,
            type_value: TypeDeclarationValue::Alias(alias_type),
        }))
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_export_stmt(&mut self, attributes: Vec<FunctionAttribute>) -> Statement {
        let export_span = self.advance_span(); // contextual `export`

        if let TokenKind::Identifier(name) = self.peek()
            && name == "type"
        {
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
                let local_span = self.advance_span();
                if matches!(self.peek(), TokenKind::Function) {
                    self.error(
                        export_span,
                        "`export local function` is not supported, use `export function`",
                    );
                    self.parse_local_function(local_span, attributes, LocalKind::Local)
                } else {
                    if !attributes.is_empty() {
                        self.error(
                            export_span,
                            "attributes are only allowed on exported functions",
                        );
                    }
                    self.parse_local_assignment(export_span, LocalKind::ExportedLocal)
                }
            }
            TokenKind::Function => {
                self.parse_local_function(export_span, attributes, LocalKind::ExportedConst)
            }
            TokenKind::Identifier(name) if name == "const" => {
                self.advance_span();
                if !attributes.is_empty() {
                    self.error(export_span, "attributes are only allowed on exported functions");
                }
                if matches!(self.peek(), TokenKind::Function) {
                    self.error(
                        export_span,
                        "`export const function` is not supported, use `export function`",
                    );
                    self.parse_local_function(export_span, Vec::new(), LocalKind::Const)
                } else {
                    self.parse_local_assignment(export_span, LocalKind::ExportedConst)
                }
            }
            _ => {
                let span = self.current_span();
                self.error(
                    span,
                    "`export` must be followed by `type`, `local`, `const`, or `function`",
                );
                return Statement::Error(export_span.merge(span));
            }
        };

        self.register_exported_names(&stmt);
        stmt
    }

    fn register_exported_names(&mut self, stmt: &Statement) {
        if let Statement::LocalAssignment(local) = stmt
            && local.is_exported
        {
            for attributed in local.names.iter() {
                self.register_exported_name(&attributed.name);
            }
        }
        if let Statement::LocalFunction(function) = stmt
            && function.is_exported
        {
            self.register_exported_name(&function.name);
        }
    }

    fn register_exported_name(&mut self, token: &Token) {
        let TokenKind::Identifier(name) = &token.kind else {
            return;
        };
        if !self.exported_names.insert(name.clone()) {
            self.error(token.span, format!("duplicate exported identifier `{name}`"));
        }
    }
}
