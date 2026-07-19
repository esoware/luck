use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::{Expression, LastStatement, Statement};
use luck_token::{Span, Token, TokenKind};

use crate::parser::Parser;

/// Build a loop-binding `Parameter` from its name and optional annotation.
fn binding_param(name: Token, type_annotation: Option<(Token, luck_ast::Type)>) -> Parameter {
    let end_span = type_annotation
        .as_ref()
        .map(|(_, binding_type)| binding_type.span())
        .unwrap_or(name.span);
    Parameter {
        span: name.span.merge(end_span),
        name,
        type_annotation,
    }
}

impl Parser {
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
                let token = self.advance();
                Some(Statement::Break(token))
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
                                self.peek_at(1),
                                TokenKind::Identifier(_) | TokenKind::Function
                            ) {
                                return Some(self.parse_type_declaration(None));
                            }
                        }
                        "export" => {
                            // `export type Name` = exported type declaration
                            if let TokenKind::Identifier(next) = self.peek_at(1)
                                && next == "type"
                            {
                                return Some(self.parse_export_type_declaration());
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
        let return_token = self.advance(); // `return`
        let start_span = return_token.span;

        let exprs = if is_block_end(self.peek()) || matches!(self.peek(), TokenKind::Semicolon) {
            Punctuated::empty()
        } else {
            self.parse_expression_list()
        };

        let semicolon = if matches!(self.peek(), TokenKind::Semicolon) {
            Some(self.advance())
        } else {
            None
        };

        let end_span = semicolon
            .as_ref()
            .map(|t| t.span)
            .or_else(|| punctuated_last_span(&exprs))
            .unwrap_or(start_span);

        LastStatement::Return(Box::new(ReturnStatement {
            span: start_span.merge(end_span),
            return_token,
            exprs,
            semicolon,
        }))
    }

    fn parse_if_statement(&mut self) -> Statement {
        let if_token = self.advance();
        self.push_context("if-statement", if_token.span);
        let condition = self.parse_expression(0);
        let then_token = self.expect_keyword(TokenKind::Then);
        let block = self.parse_block();

        let mut elseif_clauses = Vec::new();
        while matches!(self.peek(), TokenKind::ElseIf) {
            let elseif_token = self.advance();
            let elseif_condition = self.parse_expression(0);
            let elseif_then = self.expect_keyword(TokenKind::Then);
            let elseif_block = self.parse_block();
            let span = elseif_token.span.merge(elseif_block.span);
            elseif_clauses.push(ElseIfClause {
                span,
                elseif_token,
                condition: elseif_condition,
                then_token: elseif_then,
                block: elseif_block,
            });
        }

        let else_clause = if matches!(self.peek(), TokenKind::Else) {
            let else_token = self.advance();
            let else_block = self.parse_block();
            let span = else_token.span.merge(else_block.span);
            Some(ElseClause {
                span,
                else_token,
                block: else_block,
            })
        } else {
            None
        };

        let end_token = self.expect_keyword(TokenKind::End);
        self.pop_context();
        let span = if_token.span.merge(end_token.span);

        Statement::IfStatement(Box::new(IfStatement {
            span,
            if_token,
            condition,
            then_token,
            elseif_clauses,
            else_clause,
            end_token,
            block,
        }))
    }

    fn parse_while_loop(&mut self) -> Statement {
        let while_token = self.advance();
        self.push_context("while-loop", while_token.span);
        let condition = self.parse_expression(0);
        let do_token = self.expect_keyword(TokenKind::Do);
        let block = self.parse_block();
        let end_token = self.expect_keyword(TokenKind::End);
        self.pop_context();
        let span = while_token.span.merge(end_token.span);

        Statement::WhileLoop(Box::new(WhileLoop {
            span,
            while_token,
            condition,
            do_token,
            block,
            end_token,
        }))
    }

    fn parse_do_block(&mut self) -> Statement {
        let do_token = self.advance();
        self.push_context("do-block", do_token.span);
        let block = self.parse_block();
        let end_token = self.expect_keyword(TokenKind::End);
        self.pop_context();
        let span = do_token.span.merge(end_token.span);

        Statement::DoBlock(Box::new(DoBlock {
            span,
            do_token,
            block,
            end_token,
        }))
    }

    fn parse_for_statement(&mut self) -> Statement {
        let for_token = self.advance();
        self.push_context("for-loop", for_token.span);
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
        for_token: Token,
        name: Token,
        type_annotation: Option<(Token, luck_ast::Type)>,
    ) -> Statement {
        let equal = self.advance(); // `=`
        let start = self.parse_expression(0);
        let comma1 = self.expect_keyword(TokenKind::Comma);
        let limit = self.parse_expression(0);

        let comma2_and_step = if matches!(self.peek(), TokenKind::Comma) {
            let comma2 = self.advance();
            let step = self.parse_expression(0);
            Some((comma2, step))
        } else {
            None
        };

        let do_token = self.expect_keyword(TokenKind::Do);
        let block = self.parse_block();
        let end_token = self.expect_keyword(TokenKind::End);
        let span = for_token.span.merge(end_token.span);

        self.pop_context();
        Statement::NumericFor(Box::new(NumericFor {
            span,
            for_token,
            name,
            type_annotation,
            equal,
            start,
            comma1,
            limit,
            comma2_and_step,
            do_token,
            block,
            end_token,
        }))
    }

    fn parse_generic_for(
        &mut self,
        for_token: Token,
        first_name: Token,
        first_annotation: Option<(Token, luck_ast::Type)>,
    ) -> Statement {
        let mut pairs = Vec::new();
        let mut current = binding_param(first_name, first_annotation);

        while matches!(self.peek(), TokenKind::Comma) {
            let comma = self.advance();
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            // Luau: type annotation after each loop variable
            let type_annotation = self.try_parse_type_annotation();
            pairs.push((current, comma));
            current = binding_param(name, type_annotation);
        }

        let names = Punctuated::from_pairs(pairs, Some(current));

        let in_token = self.expect_keyword(TokenKind::In);
        let exprs = self.parse_expression_list();
        let do_token = self.expect_keyword(TokenKind::Do);
        let block = self.parse_block();
        let end_token = self.expect_keyword(TokenKind::End);
        let span = for_token.span.merge(end_token.span);

        self.pop_context();
        Statement::GenericFor(Box::new(GenericFor {
            span,
            for_token,
            names,
            in_token,
            exprs,
            do_token,
            block,
            end_token,
        }))
    }

    fn parse_repeat_loop(&mut self) -> Statement {
        let repeat_token = self.advance();
        self.push_context("repeat-loop", repeat_token.span);
        let block = self.parse_block();
        let until_token = self.expect_keyword(TokenKind::Until);
        let condition = self.parse_expression(0);
        let span = repeat_token.span.merge(condition.span());

        self.pop_context();
        Statement::RepeatLoop(Box::new(RepeatLoop {
            span,
            repeat_token,
            block,
            until_token,
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
        let function_token = self.advance();
        self.push_context("function declaration", function_token.span);
        let name = self.parse_func_name();
        let body = self.parse_function_body();
        self.pop_context();
        let start_span = attributes
            .first()
            .map_or(function_token.span, |attr| attr.span);
        let span = start_span.merge(body.span);

        Statement::FunctionDecl(Box::new(FunctionDecl {
            span,
            attributes,
            function_token,
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
        let mut dots = Vec::new();

        while matches!(self.peek(), TokenKind::Dot) {
            let dot = self.advance();
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            dots.push(dot);
            names.push(name);
        }

        let method = if matches!(self.peek(), TokenKind::Colon) {
            let colon = self.advance();
            let method_name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            Some((colon, method_name))
        } else {
            None
        };

        let end_span = method
            .as_ref()
            .map(|(_, n)| n.span)
            .or_else(|| names.last().map(|n| n.span))
            .unwrap_or(start_span);

        FuncName {
            span: start_span.merge(end_span),
            names,
            dots,
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
        let local_token = self.advance();

        if matches!(self.peek(), TokenKind::Function) {
            return self.parse_local_function(local_token, attributes);
        }

        if let Some(attr) = attributes.first() {
            // Luau only allows attributes on function declarations.
            self.error(
                attr.span,
                "attributes are only allowed on function declarations".to_string(),
            );
        }
        self.parse_local_assignment(local_token)
    }

    fn parse_local_function(
        &mut self,
        local_token: Token,
        attributes: Vec<FunctionAttribute>,
    ) -> Statement {
        let function_token = self.advance();
        self.push_context("local function", local_token.span);
        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        let body = self.parse_function_body();
        self.pop_context();
        let start_span = attributes
            .first()
            .map_or(local_token.span, |attr| attr.span);
        let span = start_span.merge(body.span);

        Statement::LocalFunction(Box::new(LocalFunction {
            span,
            attributes,
            local_token,
            function_token,
            name,
            body,
        }))
    }

    fn parse_local_assignment(&mut self, local_token: Token) -> Statement {
        let names = self.parse_attname_list();

        let equal_and_exprs = if matches!(self.peek(), TokenKind::Equal) {
            let equal = self.advance();
            let exprs = self.parse_expression_list();
            Some((equal, exprs))
        } else {
            None
        };

        let end_span = equal_and_exprs
            .as_ref()
            .and_then(|(_, exprs)| punctuated_last_span(exprs))
            .or_else(|| punctuated_last_name_span(&names))
            .unwrap_or(local_token.span);

        let span = local_token.span.merge(end_span);

        Statement::LocalAssignment(Box::new(LocalAssignment {
            span,
            local_token,
            names,
            equal_and_exprs,
        }))
    }

    /// Parse `goto Name`.
    fn parse_goto_statement(&mut self) -> Statement {
        let goto_token = self.advance();
        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        let span = goto_token.span.merge(name.span);
        Statement::Goto(Box::new(GotoStatement {
            span,
            goto_token,
            name,
        }))
    }

    /// Parse `:: Name ::`.
    fn parse_label_statement(&mut self) -> Statement {
        let colons_open = self.advance();
        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        let colons_close = self.expect(&TokenKind::DoubleColon).unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(TokenKind::DoubleColon, self.current_span())
        });
        let span = colons_open.span.merge(colons_close.span);
        Statement::Label(Box::new(LabelStatement {
            span,
            colons_open,
            name,
            colons_close,
        }))
    }

    /// Parse `global` declarations: `global function`, `global *`, `global namelist`.
    fn parse_global_statement(&mut self) -> Statement {
        let global_token = self.advance();

        // `global function name(...) ... end`
        if matches!(self.peek(), TokenKind::Function) {
            let function_token = self.advance();
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            let body = self.parse_function_body();
            let span = global_token.span.merge(body.span);
            return Statement::GlobalFunction(Box::new(GlobalFunction {
                span,
                global_token,
                function_token,
                name,
                body,
            }));
        }

        // `global <attrib> *` or `global *`
        if matches!(self.peek(), TokenKind::Star) {
            let star = self.advance();
            let span = global_token.span.merge(star.span);
            return Statement::GlobalStar(Box::new(GlobalStar {
                span,
                global_token,
                attrib: None,
                star,
            }));
        }

        // `global <attrib> *` - attribute before star; otherwise the
        // attribute leads an attnamelist and the shared parser handles it.
        if matches!(self.peek(), TokenKind::Less) {
            let attrib = self.parse_attribute();
            if matches!(self.peek(), TokenKind::Star) {
                let star = self.advance();
                let span = global_token.span.merge(star.span);
                return Statement::GlobalStar(Box::new(GlobalStar {
                    span,
                    global_token,
                    attrib: Some(attrib),
                    star,
                }));
            }
            let names = self.parse_attname_list_with_leading(Some(attrib));
            let end_span = punctuated_last_name_span(&names).unwrap_or(global_token.span);
            let span = global_token.span.merge(end_span);
            return Statement::GlobalDeclaration(Box::new(GlobalDeclaration {
                span,
                global_token,
                names,
            }));
        }

        // `global name [, name]*` - variable declarations
        let names = self.parse_attname_list();
        let end_span = punctuated_last_name_span(&names).unwrap_or(global_token.span);
        let span = global_token.span.merge(end_span);
        Statement::GlobalDeclaration(Box::new(GlobalDeclaration {
            span,
            global_token,
            names,
        }))
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
        let open = self.advance(); // `<`
        let name = self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(
                TokenKind::Identifier(String::new().into()),
                self.current_span(),
            )
        });
        let close = self.expect(&TokenKind::Greater).unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(TokenKind::Greater, self.current_span())
        });
        let span = open.span.merge(close.span);
        Attribute {
            span,
            open,
            name,
            close,
        }
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

        // Leading attrib applies to the first name. A second, trailing
        // attrib on the same name is an error - consume it so parsing
        // recovers instead of silently dropping it.
        let trailing_attrib = self.try_parse_attribute();
        let first_attrib = match (leading_attrib, trailing_attrib) {
            (Some(leading), Some(trailing)) => {
                self.error(
                    trailing.span,
                    "name already has a leading attribute".to_string(),
                );
                Some(leading)
            }
            (Some(leading), None) => Some(leading),
            (None, trailing) => trailing,
        };

        let mut pairs = Vec::new();
        let mut current = AttributedName {
            name: first,
            type_annotation: first_type_annotation,
            attrib: first_attrib,
        };

        while matches!(self.peek(), TokenKind::Comma) {
            let comma = self.advance();
            match self.expect_identifier() {
                Ok(name) => {
                    // Luau: type annotation after name
                    let type_annotation = self.try_parse_type_annotation();
                    let attrib = self.try_parse_attribute();
                    pairs.push((current, comma));
                    current = AttributedName {
                        name,
                        type_annotation,
                        attrib,
                    };
                }
                Err(err) => {
                    self.errors.push(err);
                    break;
                }
            }
        }

        Punctuated::from_pairs(pairs, Some(current))
    }

    fn parse_assignment_or_call(&mut self) -> Statement {
        let expr = self.parse_expression(0);

        // Luau compound assignment: `var op= expr`
        if self.version.is_luau() && is_compound_assign_op(self.peek()) {
            let op = self.advance();
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
            let mut commas = Vec::new();
            while matches!(self.peek(), TokenKind::Comma) {
                commas.push(self.advance());
                target_exprs.push(self.parse_expression(0));
            }

            let equal = self.expect(&TokenKind::Equal).unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(TokenKind::Equal, self.current_span())
            });

            let values = self.parse_expression_list();

            let target_count = target_exprs.len();
            let mut var_pairs = Vec::new();
            for (idx, target_expr) in target_exprs.into_iter().enumerate() {
                let var = expression_to_var(target_expr, self);
                if idx < target_count - 1 {
                    var_pairs.push((var, commas[idx].clone()));
                } else {
                    let targets = Punctuated::from_pairs(var_pairs, Some(var.clone()));
                    let end_span = punctuated_last_span(&values).unwrap_or(equal.span);
                    let start_span = targets.first().unwrap_or(&var).span();
                    let span = start_span.merge(end_span);

                    return Statement::Assignment(Box::new(Assignment {
                        span,
                        targets,
                        equal,
                        values,
                    }));
                }
            }
            unreachable!()
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
    fn parse_type_declaration(&mut self, export_token: Option<Token>) -> Statement {
        let type_token = self.advance(); // `type` identifier
        let start_span = export_token
            .as_ref()
            .map(|t| t.span)
            .unwrap_or(type_token.span);

        // `type function Name funcbody` - no `=`; the body is ordinary Luau
        if matches!(self.peek(), TokenKind::Function) {
            let function_token = self.advance();
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
                export_token,
                type_token,
                function_token: Some(function_token),
                name,
                generics: None,
                equal: None,
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
            Some(Box::new(self.parse_generic_type_list()))
        } else {
            None
        };

        let equal = self.expect(&TokenKind::Equal).unwrap_or_else(|err| {
            self.errors.push(err);
            Token::new(TokenKind::Equal, self.current_span())
        });

        let alias_type = self.parse_type();
        let span = start_span.merge(alias_type.span());

        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span,
            export_token,
            type_token,
            function_token: None,
            name,
            generics,
            equal: Some(equal),
            type_value: TypeDeclarationValue::Alias(alias_type),
        }))
    }

    /// Parse `export type Name ...`
    fn parse_export_type_declaration(&mut self) -> Statement {
        let export_token = self.advance(); // `export`
        self.parse_type_declaration(Some(export_token))
    }

    /// Parse `@native function ...` (Luau attributed function declaration).
    /// `@native` and friends change runtime codegen, so the attributes are
    /// kept on the AST and re-emitted - dropping them changes behavior.
    fn parse_attributed_function(&mut self) -> Statement {
        let mut attributes = Vec::new();
        while matches!(self.peek(), TokenKind::At) {
            let at_token = self.advance(); // `@`
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            attributes.push(FunctionAttribute {
                span: at_token.span.merge(name.span),
                at_token,
                name,
            });
        }

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

    /// Expect a specific keyword token, producing a contextual error with a recovery token if missing.
    fn expect_keyword(&mut self, kind: TokenKind) -> Token {
        self.expect(&kind).unwrap_or_else(|_| {
            let span = self.current_span();
            let message = if let Some((ctx, _ctx_span)) = self.context_stack.last() {
                format!("missing '{kind}' to close {ctx}")
            } else {
                format!("expected '{kind}', found {}", self.peek())
            };
            self.error(span, message);
            Token::new(kind, span)
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

fn is_compound_assign_op(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::PlusEqual
            | TokenKind::MinusEqual
            | TokenKind::StarEqual
            | TokenKind::SlashEqual
            | TokenKind::FloorDivEqual
            | TokenKind::PercentEqual
            | TokenKind::CaretEqual
            | TokenKind::DotDotEqual
    )
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
