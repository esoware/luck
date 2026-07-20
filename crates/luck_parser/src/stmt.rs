use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::{Expression, LastStatement, Statement};
use luck_token::{CompoundOp, Span, Token, TokenKind};

use crate::parser::Parser;

/// Build a loop-binding `Parameter` from its name and optional annotation.
fn binding_param(name: Token, type_annotation: Option<luck_ast::Type>) -> Parameter {
    let end_span = type_annotation
        .as_ref()
        .map(|binding_type| binding_type.span())
        .unwrap_or(name.span);
    Parameter {
        span: name.span.merge(end_span),
        name,
        type_annotation,
    }
}

/// Attribute arguments are restricted to the grammar's `literal`
/// production: nil, booleans, numbers, strings, and tables of literals
/// with plain-name or positional fields.
fn is_attribute_literal(expr: &Expression) -> bool {
    match expr {
        Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Number(_)
        | Expression::StringLiteral(_) => true,
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Named { value, .. } | Field::Positional { value, .. } => {
                is_attribute_literal(value)
            }
            Field::Bracketed { .. } => false,
        }),
        _ => false,
    }
}

impl Parser<'_> {
    /// Parse a single statement. Returns None if error recovery consumed everything.
    pub fn parse_statement(&mut self) -> Option<Statement> {
        match self.peek() {
            TokenKind::If => Some(self.parse_if_statement()),
            TokenKind::While => Some(self.parse_while_loop()),
            TokenKind::Do => Some(self.parse_do_block()),
            TokenKind::For => Some(self.parse_for_statement()),
            TokenKind::Repeat => Some(self.parse_repeat_loop()),
            TokenKind::Function => Some(self.parse_function_decl()),
            TokenKind::Local => Some(self.parse_local_statement()),
            TokenKind::Break if !self.version.break_is_last_stat_only() => {
                // 5.2+: break can appear as a regular statement
                let span = self.advance_span();
                if self.loop_depth == 0 {
                    self.error(span, "break outside a loop".to_string());
                }
                Some(Statement::Break(span))
            }
            TokenKind::Goto if self.version.has_goto() => Some(self.parse_goto_statement()),
            TokenKind::DoubleColon if self.version.has_goto() => Some(self.parse_label_statement()),
            TokenKind::Global if self.version.has_global() => Some(self.parse_global_statement()),
            // Luau `@` attribute on function declarations
            TokenKind::At if self.version.is_luau() => Some(self.parse_attributed_function()),
            TokenKind::Identifier(_) | TokenKind::LeftParen => {
                // Luau context-sensitive keywords: `type`, `export`, `continue`
                if self.version.is_luau()
                    && let TokenKind::Identifier(name) = self.peek()
                {
                    match name.as_str() {
                        "type" => {
                            // `type Name` or `type function Name` = type declaration
                            if matches!(
                                self.peek_next(),
                                TokenKind::Identifier(_) | TokenKind::Function
                            ) {
                                return Some(self.parse_type_declaration(None));
                            }
                        }
                        "export" => {
                            // `export type Name` = exported type declaration
                            if let TokenKind::Identifier(next) = self.peek_next()
                                && next == "type"
                            {
                                return Some(self.parse_export_type_declaration());
                            }
                        }
                        "const" => {
                            // `const name...` / `const function` = const
                            // declaration; `const = 1`, `const()`, `const.x`
                            // keep `const` as an ordinary identifier.
                            if matches!(
                                self.peek_next(),
                                TokenKind::Identifier(_) | TokenKind::Function
                            ) {
                                return Some(self.parse_const_statement());
                            }
                        }
                        _ => {}
                    }
                }
                Some(self.parse_assignment_or_call())
            }
            _ => {
                let span = self.current_span();
                let token = self.peek();
                let hint = match token {
                    TokenKind::End => " (extra 'end' without matching block)",
                    TokenKind::Else | TokenKind::ElseIf => {
                        " ('else'/'elseif' without matching 'if')"
                    }
                    TokenKind::Until => " ('until' without matching 'repeat')",
                    TokenKind::RightParen => " (unmatched ')')",
                    TokenKind::RightBrace => " (unmatched '}')",
                    _ => "",
                };
                self.error(span, format!("unexpected token '{token}'{hint}"));
                self.synchronize();
                Some(Statement::Error(span))
            }
        }
    }

    pub fn parse_return_statement(&mut self) -> LastStatement {
        let return_token = self.advance_span(); // `return`
        let start_span = return_token;

        let exprs = if is_block_end(self.peek()) || matches!(self.peek(), TokenKind::Semicolon) {
            Punctuated::empty()
        } else {
            self.parse_expression_list()
        };

        let semicolon = if matches!(self.peek(), TokenKind::Semicolon) {
            Some(self.advance_span())
        } else {
            None
        };

        let end_span = semicolon
            .or_else(|| punctuated_last_span(&exprs))
            .unwrap_or(start_span);

        LastStatement::Return(Box::new(ReturnStatement {
            span: start_span.merge(end_span),
            exprs,
        }))
    }

    fn parse_if_statement(&mut self) -> Statement {
        let if_token = self.advance_span();
        self.push_context("if-statement", if_token);
        let condition = self.parse_expression(0);
        self.expect_keyword(TokenKind::Then);
        let block = self.parse_block();

        let mut elseif_clauses = Vec::new();
        while matches!(self.peek(), TokenKind::ElseIf) {
            let elseif_token = self.advance_span();
            let elseif_condition = self.parse_expression(0);
            self.expect_keyword(TokenKind::Then);
            let elseif_block = self.parse_block();
            let span = elseif_token.merge(elseif_block.span);
            elseif_clauses.push(ElseIfClause {
                span,
                condition: elseif_condition,
                block: elseif_block,
            });
        }

        let else_clause = if matches!(self.peek(), TokenKind::Else) {
            let else_token = self.advance_span();
            let else_block = self.parse_block();
            let span = else_token.merge(else_block.span);
            Some(ElseClause {
                span,
                block: else_block,
            })
        } else {
            None
        };

        let end_token = self.expect_keyword(TokenKind::End);
        self.pop_context();
        let span = if_token.merge(end_token);

        Statement::IfStatement(Box::new(IfStatement {
            span,
            condition,
            elseif_clauses,
            else_clause,
            block,
        }))
    }

    fn parse_while_loop(&mut self) -> Statement {
        let while_token = self.advance_span();
        self.push_context("while-loop", while_token);
        let condition = self.parse_expression(0);
        self.expect_keyword(TokenKind::Do);
        let block = self.parse_loop_block();
        let end_token = self.expect_keyword(TokenKind::End);
        self.pop_context();
        let span = while_token.merge(end_token);

        Statement::WhileLoop(Box::new(WhileLoop {
            span,
            condition,
            block,
        }))
    }

    fn parse_do_block(&mut self) -> Statement {
        let do_token = self.advance_span();
        self.push_context("do-block", do_token);
        let block = self.parse_block();
        let end_token = self.expect_keyword(TokenKind::End);
        self.pop_context();
        let span = do_token.merge(end_token);

        Statement::DoBlock(Box::new(DoBlock { span, block }))
    }

    fn parse_for_statement(&mut self) -> Statement {
        let for_token = self.advance_span();
        self.push_context("for-loop", for_token);
        let first_name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });

        // Luau: type annotation between name and `=`/`in`
        let first_annotation = self.try_parse_type_annotation();

        // Distinguish numeric for (`name = ...`) from generic for (`namelist in ...`)
        if matches!(self.peek(), TokenKind::Equal) {
            self.parse_numeric_for(for_token, first_name, first_annotation)
        } else {
            self.parse_generic_for(for_token, first_name, first_annotation)
        }
    }

    fn parse_numeric_for(
        &mut self,
        for_token: Span,
        name: Token,
        type_annotation: Option<luck_ast::Type>,
    ) -> Statement {
        self.advance_span(); // `=`
        let start = self.parse_expression(0);
        self.expect_keyword(TokenKind::Comma);
        let limit = self.parse_expression(0);

        let step = if matches!(self.peek(), TokenKind::Comma) {
            self.advance_span();
            Some(self.parse_expression(0))
        } else {
            None
        };

        self.expect_keyword(TokenKind::Do);
        let block = self.parse_loop_block();
        let end_token = self.expect_keyword(TokenKind::End);
        let span = for_token.merge(end_token);

        self.pop_context();
        Statement::NumericFor(Box::new(NumericFor {
            span,
            name,
            type_annotation,
            start,
            limit,
            step,
            block,
        }))
    }

    fn parse_generic_for(
        &mut self,
        for_token: Span,
        first_name: Token,
        first_annotation: Option<luck_ast::Type>,
    ) -> Statement {
        let mut params = vec![binding_param(first_name, first_annotation)];

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance_span();
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            // Luau: type annotation after each loop variable
            let type_annotation = self.try_parse_type_annotation();
            params.push(binding_param(name, type_annotation));
        }

        let names = Punctuated::from_items(params);

        self.expect_keyword(TokenKind::In);
        let exprs = self.parse_expression_list();
        self.expect_keyword(TokenKind::Do);
        let block = self.parse_loop_block();
        let end_token = self.expect_keyword(TokenKind::End);
        let span = for_token.merge(end_token);

        self.pop_context();
        Statement::GenericFor(Box::new(GenericFor {
            span,
            names,
            exprs,
            block,
        }))
    }

    fn parse_repeat_loop(&mut self) -> Statement {
        let repeat_token = self.advance_span();
        self.push_context("repeat-loop", repeat_token);
        let block = self.parse_loop_block();
        self.expect_keyword(TokenKind::Until);
        let condition = self.parse_expression(0);
        let span = repeat_token.merge(condition.span());

        self.pop_context();
        Statement::RepeatLoop(Box::new(RepeatLoop {
            span,
            block,
            condition,
        }))
    }

    fn parse_function_decl(&mut self) -> Statement {
        self.parse_function_decl_with_attributes(Vec::new())
    }

    // Luau
    fn parse_function_decl_with_attributes(
        &mut self,
        attributes: Vec<FunctionAttribute>,
    ) -> Statement {
        let function_token = self.advance_span();
        self.push_context("function declaration", function_token);
        let name = self.parse_func_name();
        let body = self.parse_function_body();
        self.pop_context();
        let start_span = attributes.first().map_or(function_token, |attr| attr.span);
        let span = start_span.merge(body.span);

        Statement::FunctionDecl(Box::new(FunctionDecl {
            span,
            attributes,
            name,
            body,
        }))
    }

    fn parse_func_name(&mut self) -> FuncName {
        let first = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        let start_span = first.span;
        let mut names = vec![first];

        while matches!(self.peek(), TokenKind::Dot) {
            self.advance_span();
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            names.push(name);
        }

        let method = if matches!(self.peek(), TokenKind::Colon) {
            self.advance_span();
            let method_name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            Some(method_name)
        } else {
            None
        };

        let end_span = method
            .as_ref()
            .map(|n| n.span)
            .or_else(|| names.last().map(|n| n.span))
            .unwrap_or(start_span);

        FuncName {
            span: start_span.merge(end_span),
            names,
            method,
        }
    }

    fn parse_local_statement(&mut self) -> Statement {
        self.parse_local_statement_with_attributes(Vec::new())
    }

    // Luau
    fn parse_local_statement_with_attributes(
        &mut self,
        attributes: Vec<FunctionAttribute>,
    ) -> Statement {
        let local_token = self.advance_span();

        if matches!(self.peek(), TokenKind::Function) {
            return self.parse_local_function(local_token, attributes, false);
        }

        if let Some(attr) = attributes.first() {
            // Luau only allows attributes on function declarations.
            self.error(
                attr.span,
                "attributes are only allowed on function declarations".to_string(),
            );
        }
        self.parse_local_assignment(local_token, false)
    }

    /// Parse Luau `const` declarations, dispatched from the contextual
    /// `const` identifier: `const bindinglist '=' explist` or
    /// `const function NAME funcbody`.
    fn parse_const_statement(&mut self) -> Statement {
        let const_token = self.advance_span(); // contextual `const`
        if matches!(self.peek(), TokenKind::Function) {
            return self.parse_local_function(const_token, Vec::new(), true);
        }
        self.parse_local_assignment(const_token, true)
    }

    fn parse_local_function(
        &mut self,
        local_token: Span,
        attributes: Vec<FunctionAttribute>,
        is_const: bool,
    ) -> Statement {
        self.advance_span(); // `function`
        self.push_context("local function", local_token);
        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        let body = self.parse_function_body();
        self.pop_context();
        let start_span = attributes.first().map_or(local_token, |attr| attr.span);
        let span = start_span.merge(body.span);

        Statement::LocalFunction(Box::new(LocalFunction {
            span,
            attributes,
            name,
            body,
            is_const,
        }))
    }

    fn parse_local_assignment(&mut self, local_token: Span, is_const: bool) -> Statement {
        let names = self.parse_attname_list();

        // 5.4 §3.3.7: "A list of variables can contain at most one
        // to-be-closed variable."
        let mut seen_close = false;
        for attributed in names.iter() {
            if let Some(attrib) = &attributed.attrib
                && let TokenKind::Identifier(attr_name) = &attrib.name.kind
                && attr_name == "close"
            {
                if seen_close {
                    self.error(
                        attrib.span,
                        "multiple to-be-closed variables in local list".to_string(),
                    );
                }
                seen_close = true;
            }
        }

        let exprs = if matches!(self.peek(), TokenKind::Equal) {
            self.advance_span();
            Some(self.parse_expression_list())
        } else {
            // Grammar: `const bindinglist '=' explist` - the initializer
            // is not optional.
            if is_const {
                let span = self.current_span();
                self.error(span, "missing initializer in const declaration".to_string());
            }
            None
        };

        let end_span = exprs
            .as_ref()
            .and_then(punctuated_last_span)
            .or_else(|| punctuated_last_name_span(&names))
            .unwrap_or(local_token);

        let span = local_token.merge(end_span);

        Statement::LocalAssignment(Box::new(LocalAssignment {
            span,
            names,
            exprs,
            is_const,
        }))
    }

    /// Parse `goto Name`.
    fn parse_goto_statement(&mut self) -> Statement {
        let goto_token = self.advance_span();
        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        let span = goto_token.merge(name.span);
        Statement::Goto(Box::new(GotoStatement { span, name }))
    }

    /// Parse `:: Name ::`.
    fn parse_label_statement(&mut self) -> Statement {
        let colons_open = self.advance_span();
        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        let colons_close = self
            .expect_span(&TokenKind::DoubleColon)
            .unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
        let span = colons_open.merge(colons_close);
        Statement::Label(Box::new(LabelStatement { span, name }))
    }

    /// Parse `global` declarations: `global function`, `global *`, `global namelist`.
    fn parse_global_statement(&mut self) -> Statement {
        let global_token = self.advance_span();

        // `global function name(...) ... end`
        if matches!(self.peek(), TokenKind::Function) {
            self.advance_span(); // `function`
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            let body = self.parse_function_body();
            let span = global_token.merge(body.span);
            return Statement::GlobalFunction(Box::new(GlobalFunction { span, name, body }));
        }

        // `global <attrib> *` or `global *`
        if matches!(self.peek(), TokenKind::Star) {
            let star = self.advance_span();
            let span = global_token.merge(star);
            return Statement::GlobalStar(Box::new(GlobalStar { span, attrib: None }));
        }

        // `global <attrib> *` - attribute before star; otherwise the
        // attribute leads an attnamelist and the shared parser handles it.
        if matches!(self.peek(), TokenKind::Less) {
            let attrib = self.parse_attribute();
            if matches!(self.peek(), TokenKind::Star) {
                // 5.5 §3.3.7: only local variables can have the close
                // attribute.
                if let TokenKind::Identifier(attr_name) = &attrib.name.kind
                    && attr_name == "close"
                {
                    self.error(
                        attrib.span,
                        "only local variables can have the close attribute".to_string(),
                    );
                }
                let star = self.advance_span();
                let span = global_token.merge(star);
                return Statement::GlobalStar(Box::new(GlobalStar {
                    span,
                    attrib: Some(attrib),
                }));
            }
            let names = self.parse_attname_list_with_leading(Some(attrib));
            return self.finish_global_declaration(global_token, names);
        }

        // `global name [, name]* [= explist]` - variable declarations
        let names = self.parse_attname_list();
        self.finish_global_declaration(global_token, names)
    }

    /// Parse the optional `= explist` tail of a `global` declaration
    /// (5.5 §3.3.7: `stat ::= global attnamelist ['=' explist]`).
    fn finish_global_declaration(
        &mut self,
        global_token: Span,
        names: Punctuated<AttributedName>,
    ) -> Statement {
        // 5.5 §3.3.7: only local variables can have the close attribute.
        for attributed in names.iter() {
            if let Some(attrib) = &attributed.attrib
                && let TokenKind::Identifier(attr_name) = &attrib.name.kind
                && attr_name == "close"
            {
                self.error(
                    attrib.span,
                    "only local variables can have the close attribute".to_string(),
                );
            }
        }

        let exprs = if matches!(self.peek(), TokenKind::Equal) {
            self.advance_span();
            Some(self.parse_expression_list())
        } else {
            None
        };

        let end_span = exprs
            .as_ref()
            .and_then(punctuated_last_span)
            .or_else(|| punctuated_last_name_span(&names))
            .unwrap_or(global_token);
        let span = global_token.merge(end_span);

        Statement::GlobalDeclaration(Box::new(GlobalDeclaration { span, names, exprs }))
    }

    /// Try to parse `< Name >` attribute if the version supports it and `<` is next.
    fn try_parse_attribute(&mut self) -> Option<Attribute> {
        if self.version.has_attributes() && matches!(self.peek(), TokenKind::Less) {
            Some(self.parse_attribute())
        } else {
            None
        }
    }

    /// Parse `< Name >` attribute (assumes `<` is current token).
    fn parse_attribute(&mut self) -> Attribute {
        let open = self.advance_span(); // `<`
        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        // 5.4 §3.3.7: "There are two possible attributes"; real Lua
        // rejects anything else at parse time.
        if let TokenKind::Identifier(attr_name) = &name.kind
            && !attr_name.is_empty()
            && attr_name != "const"
            && attr_name != "close"
        {
            self.error(name.span, format!("unknown attribute '{attr_name}'"));
        }
        let close = self.expect_span(&TokenKind::Greater).unwrap_or_else(|err| {
            self.errors.push(err);
            self.current_span()
        });
        let span = open.merge(close);
        Attribute { span, name }
    }

    /// Parse attnamelist: `[attrib] Name [attrib] { ',' Name [attrib] }`
    /// In Lua 5.5, attribute can appear before the first name.
    /// In Lua 5.4, attributes only follow names.
    fn parse_attname_list(&mut self) -> Punctuated<AttributedName> {
        // Lua 5.5: optional leading attribute before first name
        let leading_attrib =
            if self.version.has_leading_attributes() && matches!(self.peek(), TokenKind::Less) {
                Some(self.parse_attribute())
            } else {
                None
            };
        self.parse_attname_list_with_leading(leading_attrib)
    }

    /// Attnamelist continuation once any leading attribute has been parsed
    /// (also entered directly by `global <attrib> name...`).
    fn parse_attname_list_with_leading(
        &mut self,
        leading_attrib: Option<Attribute>,
    ) -> Punctuated<AttributedName> {
        let first = match self.expect_identifier() {
            Ok(t) => t,
            Err(err) => {
                self.errors.push(err);
                return Punctuated::empty();
            }
        };

        // Luau: type annotation after name
        let first_type_annotation = self.try_parse_type_annotation();

        // 5.5 §3.3.7: a prefixed attribute applies to ALL names in the
        // list, so it is distributed onto every name that lacks its own
        // trailing attribute (the emitted trailing form is equivalent).
        // A name carrying both is an error - consume the trailing one so
        // parsing recovers instead of silently dropping it.
        let resolve_attrib =
            |parser: &mut Self, leading: Option<&Attribute>, trailing: Option<Attribute>| match (
                leading, trailing,
            ) {
                (Some(_), Some(trailing)) => {
                    parser.error(
                        trailing.span,
                        "name already has a leading attribute".to_string(),
                    );
                    leading.cloned()
                }
                (Some(leading), None) => Some(leading.clone()),
                (None, trailing) => trailing,
            };

        let trailing_attrib = self.try_parse_attribute();
        let first_attrib = resolve_attrib(self, leading_attrib.as_ref(), trailing_attrib);

        let mut names = vec![AttributedName {
            name: first,
            type_annotation: first_type_annotation,
            attrib: first_attrib,
        }];

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance_span();
            match self.expect_identifier() {
                Ok(name) => {
                    // Luau: type annotation after name
                    let type_annotation = self.try_parse_type_annotation();
                    let trailing = self.try_parse_attribute();
                    let attrib = resolve_attrib(self, leading_attrib.as_ref(), trailing);
                    names.push(AttributedName {
                        name,
                        type_annotation,
                        attrib,
                    });
                }
                Err(err) => {
                    self.errors.push(err);
                    break;
                }
            }
        }

        Punctuated::from_items(names)
    }

    fn parse_assignment_or_call(&mut self) -> Statement {
        let expr = self.parse_expression(0);

        // Luau compound assignment: `var op= expr`
        if self.version.is_luau()
            && let Some(op) = CompoundOp::from_token_kind(self.peek())
        {
            self.advance_span();
            let rhs = self.parse_expression(0);
            let var = expression_to_var(expr, self);
            let span = var.span().merge(rhs.span());
            return Statement::CompoundAssignment(Box::new(CompoundAssignment {
                span,
                var,
                op,
                expr: rhs,
            }));
        }

        // Check for assignment: `varlist = explist`
        if matches!(self.peek(), TokenKind::Comma | TokenKind::Equal) {
            let mut target_exprs = vec![expr];
            while matches!(self.peek(), TokenKind::Comma) {
                self.advance_span();
                target_exprs.push(self.parse_expression(0));
            }

            let equal = self.expect_span(&TokenKind::Equal).unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });

            let values = self.parse_expression_list();

            let vars: Vec<Var> = target_exprs
                .into_iter()
                .map(|target_expr| expression_to_var(target_expr, self))
                .collect();
            let targets = Punctuated::from_items(vars);
            let end_span = punctuated_last_span(&values).unwrap_or(equal);
            let start_span = targets.first().map_or(equal, |var| var.span());
            let span = start_span.merge(end_span);

            return Statement::Assignment(Box::new(Assignment {
                span,
                targets,
                values,
            }));
        }

        match expr {
            Expression::FunctionCall(call) => {
                let span = call.span;
                Statement::FunctionCall(Box::new(FunctionCallStmt { span, call: *call }))
            }
            _ => {
                let span = expr.span();
                self.error(
                    span,
                    "expected function call or assignment (expressions are not valid statements)"
                        .to_string(),
                );
                Statement::Error(span)
            }
        }
    }

    /// Parse Luau type declaration: `type Name ['<' ... '>'] '=' TYPE`
    /// Also handles `type function Name funcbody`.
    fn parse_type_declaration(&mut self, export_token: Option<Span>) -> Statement {
        let type_token = self.advance_span(); // `type` identifier
        let start_span = export_token.unwrap_or(type_token);

        // `type function Name funcbody` - no `=`; the body is ordinary Luau
        if matches!(self.peek(), TokenKind::Function) {
            self.advance_span(); // `function`
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            let body = self.parse_function_body();
            let span = start_span.merge(body.span);
            return Statement::TypeDeclaration(Box::new(TypeDeclaration {
                span,
                is_exported: export_token.is_some(),
                name,
                generics: None,
                type_value: TypeDeclarationValue::TypeFunction(Box::new(body)),
            }));
        }

        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });

        let generics = if matches!(self.peek(), TokenKind::Less) {
            Some(Box::new(self.parse_generic_type_list(true)))
        } else {
            None
        };

        if let Err(err) = self.expect_span(&TokenKind::Equal) {
            self.errors.push(err);
        }

        let alias_type = self.parse_type();
        let span = start_span.merge(alias_type.span());

        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span,
            is_exported: export_token.is_some(),
            name,
            generics,
            type_value: TypeDeclarationValue::Alias(alias_type),
        }))
    }

    /// Parse `export type Name ...`
    fn parse_export_type_declaration(&mut self) -> Statement {
        let export_token = self.advance_span(); // `export`
        self.parse_type_declaration(Some(export_token))
    }

    /// Parse `@native function ...` (Luau attributed function declaration).
    /// `@native` and friends change runtime codegen, so the attributes are
    /// kept on the AST and re-emitted - dropping them changes behavior.
    /// Parse a run of Luau attributes covering both grammar forms:
    /// `attribute ::= '@' NAME | '@[' parattr {',' parattr} ']'` with
    /// `parattr ::= NAME [pars]` and literal-only arguments.
    pub(crate) fn parse_function_attributes(&mut self) -> Vec<FunctionAttribute> {
        let mut attributes = Vec::new();
        while matches!(self.peek(), TokenKind::At) {
            let at_token = self.advance_span(); // `@`
            if matches!(self.peek(), TokenKind::LeftBracket) {
                self.advance_span(); // `[`
                loop {
                    let name = self.expect_identifier().unwrap_or_else(|err| {
                        self.errors.push(err);
                        Token::new(
                            TokenKind::Identifier(String::new().into()),
                            self.current_span(),
                        )
                    });
                    let args = self.parse_attribute_args();
                    self.validate_function_attribute(&name, args.is_some(), &attributes);
                    let end_span = args
                        .as_ref()
                        .and_then(punctuated_last_span)
                        .unwrap_or(name.span);
                    attributes.push(FunctionAttribute {
                        span: at_token.merge(end_span),
                        name,
                        args,
                    });
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance_span();
                    } else {
                        break;
                    }
                }
                if let Err(err) = self.expect_span(&TokenKind::RightBracket) {
                    self.errors.push(err);
                }
            } else {
                let name = self.expect_identifier().unwrap_or_else(|err| {
                    self.errors.push(err);
                    Token::new(
                        TokenKind::Identifier(String::new().into()),
                        self.current_span(),
                    )
                });
                self.validate_function_attribute(&name, false, &attributes);
                attributes.push(FunctionAttribute {
                    span: at_token.merge(name.span),
                    name,
                    args: None,
                });
            }
        }
        attributes
    }

    /// Parse the optional `pars` of a parenthesized attribute:
    /// `pars ::= '(' [litlist] ')' | littable | STRING`. The string and
    /// table sugar canonicalize to a one-element argument list.
    fn parse_attribute_args(&mut self) -> Option<Punctuated<Expression>> {
        match self.peek() {
            TokenKind::LeftParen => {
                self.advance_span();
                let args = if matches!(self.peek(), TokenKind::RightParen) {
                    Punctuated::empty()
                } else {
                    self.parse_expression_list()
                };
                if let Err(err) = self.expect_span(&TokenKind::RightParen) {
                    self.errors.push(err);
                }
                for expr in args.iter() {
                    if !is_attribute_literal(expr) {
                        self.error(
                            expr.span(),
                            "attribute arguments must be literals".to_string(),
                        );
                    }
                }
                Some(args)
            }
            TokenKind::StringLiteral(_) => {
                let token = self.advance();
                let TokenKind::StringLiteral(text) = token.kind else {
                    unreachable!("peeked kind");
                };
                Some(Punctuated::from_item(Expression::StringLiteral(Literal {
                    text,
                    span: token.span,
                })))
            }
            TokenKind::LeftBrace => {
                let table = self.parse_table_constructor();
                let expr = Expression::TableConstructor(Box::new(table));
                if !is_attribute_literal(&expr) {
                    self.error(
                        expr.span(),
                        "attribute arguments must be literals".to_string(),
                    );
                }
                Some(Punctuated::from_item(expr))
            }
            _ => None,
        }
    }

    /// Luau validates attributes at parse time: only
    /// checked/native/deprecated exist, duplicates are errors, and only
    /// `deprecated` accepts arguments.
    fn validate_function_attribute(
        &mut self,
        name: &Token,
        has_args: bool,
        previous: &[FunctionAttribute],
    ) {
        let TokenKind::Identifier(attr_name) = &name.kind else {
            return;
        };
        if attr_name.is_empty() {
            return;
        }
        if !matches!(attr_name.as_str(), "checked" | "native" | "deprecated") {
            self.error(name.span, format!("invalid attribute '@{attr_name}'"));
            return;
        }
        if has_args && attr_name != "deprecated" {
            self.error(
                name.span,
                format!("attribute '@{attr_name}' does not take arguments"),
            );
        }
        if previous.iter().any(|prev| {
            matches!(&prev.name.kind, TokenKind::Identifier(existing) if existing == attr_name)
        }) {
            self.error(name.span, format!("duplicate attribute '@{attr_name}'"));
        }
    }

    fn parse_attributed_function(&mut self) -> Statement {
        let attributes = self.parse_function_attributes();

        // The next token should be `function` (global) or `local` (local function)
        if matches!(self.peek(), TokenKind::Local) {
            return self.parse_local_statement_with_attributes(attributes);
        }

        if matches!(self.peek(), TokenKind::Function) {
            return self.parse_function_decl_with_attributes(attributes);
        }

        let span = self.current_span();
        self.error(
            span,
            "expected 'function' or 'local' after attribute".to_string(),
        );
        Statement::Error(span)
    }

    /// Expect a specific keyword token, producing a contextual error with a recovery span if missing.
    fn expect_keyword(&mut self, kind: TokenKind) -> Span {
        self.expect_span(&kind).unwrap_or_else(|_| {
            let span = self.current_span();
            let message = if let Some((ctx, _ctx_span)) = self.context_stack.last() {
                format!("missing '{kind}' to close {ctx}")
            } else {
                format!("expected '{kind}', found {}", self.peek())
            };
            self.error(span, message);
            span
        })
    }
}

/// Convert an Expression to a Var. If the expression isn't a valid assignment target,
/// record an error and return a synthetic Var::Name.
fn expression_to_var(expr: Expression, parser: &mut Parser) -> Var {
    match expr {
        Expression::Var(var) => *var,
        _ => {
            let span = expr.span();
            parser.error(span, "invalid assignment target".to_string());
            Var::Name(Token::new(
                TokenKind::Identifier(String::new().into()),
                span,
            ))
        }
    }
}

/// Whether this token ends a block.
fn is_block_end(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::End | TokenKind::Else | TokenKind::ElseIf | TokenKind::Until | TokenKind::Eof
    )
}

/// Get the span of the last expression in a Punctuated list.
fn punctuated_last_span(punct: &Punctuated<Expression>) -> Option<Span> {
    punct.last_item().map(|e| e.span())
}

/// Get the span of the last declared name in an attname list.
fn punctuated_last_name_span(punct: &Punctuated<AttributedName>) -> Option<Span> {
    punct.last_item().map(|attributed| attributed.name.span)
}
