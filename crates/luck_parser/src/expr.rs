use luck_ast::Expression;
use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_token::{Assoc, BinOp, Span, Token, TokenKind, UNARY_PRECEDENCE, UnOp};

use crate::parser::Parser;

impl Parser<'_> {
    /// Pratt expression parser. The left side grows iteratively; only the
    /// right-hand operand recurses, avoiding stack overflow on long chains.
    #[inline]
    pub fn parse_expression(&mut self, min_precedence: u8) -> Expression {
        let mut left = self.parse_prefix();

        while let Some(op) = BinOp::from_token_kind(self.peek()) {
            let (precedence, assoc) = op.precedence();
            if precedence < min_precedence {
                break;
            }
            let op_span = self.advance_span();
            let right_min = if assoc == Assoc::Right {
                precedence
            } else {
                precedence + 1
            };
            // Right-associative chains (`..`, `^`) recurse once per operator;
            // without a guard a long chain overflows the stack (left-assoc
            // chains stay iterative and never accumulate depth here).
            if let Err(err) = self.enter_depth() {
                self.errors.push(err);
                let span = left.span().merge(op_span);
                return Expression::BinaryOp(Box::new(BinaryOp {
                    span,
                    left,
                    op,
                    op_span,
                    right: Expression::Error(op_span),
                }));
            }
            let right = self.parse_expression(right_min);
            self.exit_depth();
            let span = left.span().merge(right.span());
            left = Expression::BinaryOp(Box::new(BinaryOp {
                span,
                left,
                op,
                op_span,
                right,
            }));
        }

        left
    }

    /// Parse a prefix expression: unary operator or primary.
    fn parse_prefix(&mut self) -> Expression {
        if self.peek().is_unary_op() {
            let op = UnOp::from_token_kind(self.peek())
                .expect("is_unary_op and UnOp::from_token_kind cover the same kinds");
            let op_span = self.advance_span();
            // Depth-limit unary chains (e.g. `not not not ... x`) to prevent stack overflow
            if let Err(err) = self.enter_depth() {
                self.errors.push(err);
                return Expression::Error(op_span);
            }
            let operand = self.parse_expression(UNARY_PRECEDENCE);
            self.exit_depth();
            let span = op_span.merge(operand.span());
            return Expression::UnaryOp(Box::new(UnaryOp {
                span,
                op,
                op_span,
                operand,
            }));
        }
        let primary = self.parse_primary_expression();
        // Luau type assertions (`expr :: Type`) apply to ANY simpleexp, not just
        // prefix expressions, so they're handled here rather than inside the
        // prefix-only suffix loop. This lets `1 :: number`, `{} :: Foo`,
        // `f() :: T`, etc. parse - matching Luau's `asexp = simpleexp ['::' Type]`.
        self.parse_type_assertions(primary)
    }

    /// Wrap a just-parsed primary in any trailing Luau `:: Type` assertions.
    /// A no-op outside Luau. Loops so chained `x :: A :: B` parses left to right.
    fn parse_type_assertions(&mut self, mut expr: Expression) -> Expression {
        while self.version.is_luau() && matches!(self.peek(), TokenKind::DoubleColon) {
            let double_colon = self.advance_span();
            let type_annotation = self.parse_type();
            let span = expr.span().merge(type_annotation.span());
            expr = Expression::TypeCast(Box::new(TypeCast {
                span,
                expr,
                double_colon,
                type_annotation,
            }));
        }
        expr
    }

    /// Parse a primary expression and then any suffix chain.
    fn parse_primary_expression(&mut self) -> Expression {
        match self.peek() {
            TokenKind::Nil => Expression::Nil(self.advance_span()),
            TokenKind::False => Expression::False(self.advance_span()),
            TokenKind::True => Expression::True(self.advance_span()),
            TokenKind::Number(_) => Expression::Number(self.advance()),
            TokenKind::StringLiteral(_) => Expression::StringLiteral(self.advance()),
            TokenKind::DotDotDot => Expression::VarArg(self.advance_span()),
            TokenKind::Function => self.parse_function_def(),
            TokenKind::LeftBrace => self.parse_table_constructor_expr(),
            TokenKind::LeftParen => {
                let open = self.advance_span();
                if let Err(err) = self.enter_depth() {
                    self.errors.push(err);
                    return Expression::Error(open);
                }
                let expr = self.parse_expression(0);
                self.exit_depth();
                let close = self
                    .expect_span(&TokenKind::RightParen)
                    .unwrap_or_else(|err| {
                        self.errors.push(err);
                        self.current_span()
                    });
                let span = open.merge(close);
                let paren_expr = Expression::Parenthesized(Box::new(ParenExpression {
                    span,
                    parens: ContainedSpan { open, close },
                    expr,
                }));
                self.parse_suffixes(paren_expr)
            }
            // Luau if-expression: `if cond then expr {elseif cond then expr} else expr`
            TokenKind::If if self.version.is_luau() => self.parse_if_expression(),
            // Luau interpolated string
            TokenKind::InterpBegin(_) => self.parse_interpolated_string(),
            TokenKind::Identifier(_) => {
                let name_token = self.advance();
                let var_expr = Expression::Var(Box::new(Var::Name(name_token)));
                self.parse_suffixes(var_expr)
            }
            _ => {
                let span = self.current_span();
                let token = self.peek();
                let hint = match token {
                    TokenKind::End | TokenKind::Else | TokenKind::ElseIf | TokenKind::Until => {
                        " (possible missing expression before keyword)"
                    }
                    TokenKind::RightParen => " (unmatched ')')",
                    TokenKind::RightBrace => " (unmatched '}')",
                    TokenKind::RightBracket => " (unmatched ']')",
                    TokenKind::Equal => " (use '==' for comparison, not '=')",
                    _ => "",
                };
                self.error(
                    span,
                    format!("unexpected token '{token}' in expression{hint}"),
                );
                Expression::Error(span)
            }
        }
    }

    /// Iteratively parse suffix chains: `.name`, `[expr]`, `:name(args)`, `(args)`, `{table}`, `"string"`.
    fn parse_suffixes(&mut self, mut expr: Expression) -> Expression {
        loop {
            match self.peek() {
                TokenKind::Dot => {
                    let dot = self.advance_span();
                    let name = self.expect_identifier().unwrap_or_else(|err| {
                        self.errors.push(err);
                        Token::new(
                            TokenKind::Identifier(String::new().into()),
                            self.current_span(),
                        )
                    });
                    let span = expr.span().merge(name.span);
                    expr = Expression::Var(Box::new(Var::FieldAccess(Box::new(FieldAccess {
                        span,
                        prefix: expr,
                        dot,
                        name,
                    }))));
                }
                TokenKind::LeftBracket => {
                    let open = self.advance_span();
                    let index = self.parse_expression(0);
                    let close = self
                        .expect_span(&TokenKind::RightBracket)
                        .unwrap_or_else(|err| {
                            self.errors.push(err);
                            self.current_span()
                        });
                    let span = expr.span().merge(close);
                    expr = Expression::Var(Box::new(Var::Index(Box::new(IndexExpression {
                        span,
                        prefix: expr,
                        brackets: ContainedSpan { open, close },
                        index,
                    }))));
                }
                TokenKind::Colon => {
                    let colon = self.advance_span();
                    let method_name = self.expect_identifier().unwrap_or_else(|err| {
                        self.errors.push(err);
                        Token::new(
                            TokenKind::Identifier(String::new().into()),
                            self.current_span(),
                        )
                    });
                    let args = self.parse_function_args();
                    let args_span = function_args_span(&args);
                    let span = expr.span().merge(args_span);
                    expr = Expression::FunctionCall(Box::new(FunctionCall {
                        span,
                        callee: expr,
                        args,
                        method: Some((colon, method_name)),
                    }));
                }
                TokenKind::LeftParen | TokenKind::LeftBrace | TokenKind::StringLiteral(_) => {
                    let args = self.parse_function_args();
                    let args_span = function_args_span(&args);
                    let span = expr.span().merge(args_span);
                    expr = Expression::FunctionCall(Box::new(FunctionCall {
                        span,
                        callee: expr,
                        args,
                        method: None,
                    }));
                }
                // Type assertions (`expr :: Type`) are handled by
                // `parse_type_assertions` after the primary, so they apply to
                // all simpleexps (literals, tables, calls), not just suffix chains.
                _ => break,
            }
        }
        expr
    }

    /// Parse a Luau if-expression: `if cond then expr {elseif cond then expr} else expr`
    fn parse_if_expression(&mut self) -> Expression {
        let if_token = self.advance_span(); // `if`
        if let Err(err) = self.enter_depth() {
            self.errors.push(err);
            return Expression::Error(if_token);
        }
        let condition = self.parse_expression(0);
        let then_token = self.expect_span(&TokenKind::Then).unwrap_or_else(|err| {
            self.errors.push(err);
            self.current_span()
        });
        let then_expr = self.parse_expression(0);

        let mut elseif_clauses = Vec::new();
        while matches!(self.peek(), TokenKind::ElseIf) {
            let elseif_token = self.advance_span();
            let elseif_condition = self.parse_expression(0);
            let elseif_then = self.expect_span(&TokenKind::Then).unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
            let elseif_expr = self.parse_expression(0);
            let span = elseif_token.merge(elseif_expr.span());
            elseif_clauses.push(ElseIfExprClause {
                span,
                elseif_token,
                condition: elseif_condition,
                then_token: elseif_then,
                expr: elseif_expr,
            });
        }

        let else_token = self.expect_span(&TokenKind::Else).unwrap_or_else(|err| {
            self.errors.push(err);
            self.current_span()
        });
        let else_expr = self.parse_expression(0);
        self.exit_depth();
        let span = if_token.merge(else_expr.span());

        Expression::IfExpression(Box::new(IfExpression {
            span,
            if_token,
            condition,
            then_token,
            then_expr,
            elseif_clauses,
            else_token,
            else_expr,
        }))
    }

    /// Parse a Luau interpolated string: InterpBegin {expr InterpMid} expr InterpEnd
    fn parse_interpolated_string(&mut self) -> Expression {
        let begin_token = self.advance(); // InterpBegin
        let start_span = begin_token.span;
        let mut segments = Vec::new();

        // InterpBegin is always followed by either InterpEnd (no expressions) or an expression
        match self.peek() {
            TokenKind::InterpEnd(_) => {
                // Plain string with no interpolations: InterpBegin("") + InterpEnd("text")
                let end_token = self.advance();
                segments.push(InterpSegment {
                    literal: begin_token,
                    expr: None,
                });
                let span = start_span.merge(end_token.span);
                segments.push(InterpSegment {
                    literal: end_token,
                    expr: None,
                });
                Expression::InterpolatedString(Box::new(InterpolatedString { span, segments }))
            }
            _ => {
                // Has interpolation expressions
                let expr = self.parse_expression(0);
                segments.push(InterpSegment {
                    literal: begin_token,
                    expr: Some(expr),
                });

                // Parse InterpMid segments
                loop {
                    match self.peek() {
                        TokenKind::InterpMid(_) => {
                            let mid_token = self.advance();
                            let expr = self.parse_expression(0);
                            segments.push(InterpSegment {
                                literal: mid_token,
                                expr: Some(expr),
                            });
                        }
                        TokenKind::InterpEnd(_) => {
                            let end_token = self.advance();
                            let span = start_span.merge(end_token.span);
                            segments.push(InterpSegment {
                                literal: end_token,
                                expr: None,
                            });
                            return Expression::InterpolatedString(Box::new(InterpolatedString {
                                span,
                                segments,
                            }));
                        }
                        _ => {
                            // Error: unexpected token in interpolated string
                            let span = self.current_span();
                            self.error(
                                span,
                                format!(
                                    "expected interpolated string continuation, found {}",
                                    self.peek()
                                ),
                            );
                            return Expression::InterpolatedString(Box::new(InterpolatedString {
                                span: start_span.merge(span),
                                segments,
                            }));
                        }
                    }
                }
            }
        }
    }

    /// Parse function call arguments: `(explist)`, `{table}`, or `"string"`.
    fn parse_function_args(&mut self) -> FunctionArgs {
        match self.peek() {
            TokenKind::LeftParen => {
                let open = self.advance_span();
                let args = if matches!(self.peek(), TokenKind::RightParen) {
                    Punctuated::empty()
                } else {
                    self.parse_expression_list()
                };
                let close = self
                    .expect_span(&TokenKind::RightParen)
                    .unwrap_or_else(|err| {
                        self.errors.push(err);
                        self.current_span()
                    });
                FunctionArgs::Parenthesized {
                    parens: ContainedSpan { open, close },
                    args,
                }
            }
            TokenKind::LeftBrace => {
                let table = self.parse_table_constructor();
                FunctionArgs::TableConstructor(Box::new(table))
            }
            TokenKind::StringLiteral(_) => {
                let token = self.advance();
                FunctionArgs::StringLiteral(token)
            }
            _ => {
                let span = self.current_span();
                self.error(span, "expected function arguments".to_string());
                FunctionArgs::Parenthesized {
                    parens: ContainedSpan {
                        open: span,
                        close: span,
                    },
                    args: Punctuated::empty(),
                }
            }
        }
    }

    /// Parse `function(params) block end`.
    fn parse_function_def(&mut self) -> Expression {
        let function_token = self.advance_span(); // `function`
        let body = self.parse_function_body();
        let span = function_token.merge(body.span);
        Expression::FunctionDef(Box::new(FunctionDef {
            span,
            function_token,
            body,
        }))
    }

    /// Parse function body: `[<generics>] (params) block end`.
    pub fn parse_function_body(&mut self) -> FunctionBody {
        // Luau: generic list before the parens - `function f<T>(x: T)`
        let generics = if self.version.is_luau() && matches!(self.peek(), TokenKind::Less) {
            Some(Box::new(self.parse_generic_type_list()))
        } else {
            None
        };

        let open = self
            .expect_span(&TokenKind::LeftParen)
            .unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
        let start_span = generics.as_ref().map(|list| list.span).unwrap_or(open);

        if let Err(err) = self.enter_depth() {
            self.errors.push(err);
            return FunctionBody {
                span: start_span,
                generics,
                params_parens: ContainedSpan {
                    open,
                    close: start_span,
                },
                params: Punctuated::empty(),
                vararg: None,
                return_type: None,
                block: luck_ast::Block {
                    span: start_span,
                    stmts: Vec::new(),
                    last_stmt: None,
                },
                end_token: start_span,
            };
        }

        let mut params = Punctuated::<Parameter>::empty();
        let mut vararg = None;

        if !matches!(self.peek(), TokenKind::RightParen) {
            if matches!(self.peek(), TokenKind::DotDotDot) {
                let dots = self.advance_span();
                // Lua 5.5: named varargs `...name`
                let vararg_name = if self.version.has_named_varargs() && self.check_identifier() {
                    Some(self.advance())
                } else {
                    None
                };
                // Luau: vararg type annotation `...: type`
                let vararg_type = self.try_parse_type_annotation();
                let end_span = vararg_type
                    .as_ref()
                    .map(|(_, annotation)| annotation.span())
                    .or(vararg_name.as_ref().map(|n| n.span))
                    .unwrap_or(dots);
                vararg = Some(VarArgParam {
                    span: dots.merge(end_span),
                    dots,
                    name: vararg_name,
                    type_annotation: vararg_type,
                });
            } else if self.check_identifier() {
                let first_name = self.advance();
                let type_ann = self.try_parse_type_annotation();
                let first_param = Parameter {
                    span: first_name.span.merge(
                        type_ann
                            .as_ref()
                            .map(|(_, annotation)| annotation.span())
                            .unwrap_or(first_name.span),
                    ),
                    name: first_name,
                    type_annotation: type_ann,
                };

                let mut current = Some(first_param);
                while matches!(self.peek(), TokenKind::Comma) {
                    let comma = self.advance_span();
                    if matches!(self.peek(), TokenKind::DotDotDot) {
                        // Vararg after last named param
                        params.push(
                            current
                                .take()
                                .expect("current is always Some at loop entry"),
                            Some(comma),
                        );
                        let dots = self.advance_span();
                        // Lua 5.5: named varargs `...name`
                        let vararg_name =
                            if self.version.has_named_varargs() && self.check_identifier() {
                                Some(self.advance())
                            } else {
                                None
                            };
                        let vararg_type = self.try_parse_type_annotation();
                        let end_span = vararg_type
                            .as_ref()
                            .map(|(_, annotation)| annotation.span())
                            .or(vararg_name.as_ref().map(|n| n.span))
                            .unwrap_or(dots);
                        vararg = Some(VarArgParam {
                            span: dots.merge(end_span),
                            dots,
                            name: vararg_name,
                            type_annotation: vararg_type,
                        });
                        break;
                    }
                    let name = self.expect_identifier().unwrap_or_else(|err| {
                        self.errors.push(err);
                        Token::new(
                            TokenKind::Identifier(String::new().into()),
                            self.current_span(),
                        )
                    });
                    let type_ann = self.try_parse_type_annotation();
                    let param = Parameter {
                        span: name.span.merge(
                            type_ann
                                .as_ref()
                                .map(|(_, annotation)| annotation.span())
                                .unwrap_or(name.span),
                        ),
                        name,
                        type_annotation: type_ann,
                    };
                    params.push(
                        current
                            .take()
                            .expect("current is always Some at loop entry"),
                        Some(comma),
                    );
                    current = Some(param);
                }

                if let Some(last_param) = current {
                    params.push(last_param, None);
                }
            }
        }

        let close = self
            .expect_span(&TokenKind::RightParen)
            .unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });

        // Luau: optional return type annotation after `)`
        let return_type = self.try_parse_type_annotation();

        let block = self.parse_block();

        let end_token = self.expect_span(&TokenKind::End).unwrap_or_else(|err| {
            self.errors.push(err);
            self.current_span()
        });

        self.exit_depth();

        let span = start_span.merge(end_token);

        FunctionBody {
            span,
            generics,
            params_parens: ContainedSpan { open, close },
            params,
            vararg,
            return_type,
            block,
            end_token,
        }
    }

    /// Parse a table constructor: `{ [fieldlist] }`.
    pub fn parse_table_constructor(&mut self) -> TableConstructor {
        let open = self.advance_span(); // `{`
        let start_span = open;

        if let Err(err) = self.enter_depth() {
            self.errors.push(err);
            return TableConstructor {
                span: start_span,
                braces: ContainedSpan {
                    open,
                    close: start_span,
                },
                fields: Vec::new(),
            };
        }

        let mut fields = Vec::new();

        // A leading separator is always an error in every Lua version
        // (`{;}`, `{,}`, `{, 1}` are all rejected by PUC Lua and Luau).
        // Consume it so parsing recovers.
        if matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon) {
            let span = self.current_span();
            self.error(
                span,
                "unexpected separator in table constructor".to_string(),
            );
            self.advance_span();
        }

        loop {
            if matches!(self.peek(), TokenKind::RightBrace | TokenKind::Eof) {
                break;
            }

            let field = self.parse_field();
            let separator = if matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon) {
                Some(self.advance_span())
            } else {
                None
            };
            let has_separator = separator.is_some();
            fields.push((field, separator));

            if !has_separator {
                break;
            }
        }

        self.exit_depth();

        let close = self
            .expect_span(&TokenKind::RightBrace)
            .unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
        let span = start_span.merge(close);

        TableConstructor {
            span,
            braces: ContainedSpan { open, close },
            fields,
        }
    }

    fn parse_table_constructor_expr(&mut self) -> Expression {
        let table = self.parse_table_constructor();
        Expression::TableConstructor(Box::new(table))
    }

    /// Parse a single table field.
    fn parse_field(&mut self) -> Field {
        // `[expr] = expr`
        if matches!(self.peek(), TokenKind::LeftBracket) {
            let open = self.advance_span();
            let key = self.parse_expression(0);
            let close = self
                .expect_span(&TokenKind::RightBracket)
                .unwrap_or_else(|err| {
                    self.errors.push(err);
                    self.current_span()
                });
            let equal = self.expect_span(&TokenKind::Equal).unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
            let value = self.parse_expression(0);
            let span = open.merge(value.span());
            return Field::Bracketed {
                span,
                brackets: ContainedSpan { open, close },
                key,
                equal,
                value,
            };
        }

        // `Name = expr` - need lookahead: identifier followed by `=`
        if self.check_identifier() && matches!(self.peek_next(), TokenKind::Equal) {
            let name = self.advance();
            let equal = self.advance_span();
            let value = self.parse_expression(0);
            let span = name.span.merge(value.span());
            return Field::Named {
                span,
                name,
                equal,
                value,
            };
        }

        // Positional: just an expression
        let value = self.parse_expression(0);
        let span = value.span();
        Field::Positional { span, value }
    }
}

/// Get the ending span of function arguments.
fn function_args_span(args: &FunctionArgs) -> Span {
    match args {
        FunctionArgs::Parenthesized { parens, .. } => parens.open.merge(parens.close),
        FunctionArgs::TableConstructor(t) => t.span,
        FunctionArgs::StringLiteral(t) => t.span,
    }
}
