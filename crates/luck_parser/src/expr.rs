use luck_ast::Expression;
use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_token::{Assoc, BinOp, Span, Token, TokenKind, UNARY_PRECEDENCE, UnOp};

use crate::parser::Parser;

/// Split a payload-carrying token (number or string literal) into a
/// `Literal`; the caller has already matched the kind.
fn literal_from(token: Token) -> Literal {
    let (TokenKind::Number(text) | TokenKind::StringLiteral(text)) = token.kind else {
        unreachable!("caller matched a literal token kind");
    };
    Literal {
        text,
        span: token.span,
    }
}

fn number_from(token: Token) -> Expression {
    let literal = literal_from(token);
    if literal.text.ends_with('i') {
        Expression::Integer(literal)
    } else {
        Expression::Number(literal)
    }
}

impl Parser<'_> {
    /// Pratt expression parser. The left side grows iteratively; only the
    /// right-hand operand recurses, avoiding stack overflow on long chains.
    #[inline]
    pub(crate) fn parse_expression(&mut self, min_precedence: u8) -> Expression {
        let mut left = self.parse_prefix();

        while let Some(op) = BinOp::from_token_kind(self.peek()) {
            // Luau has no bitwise operators; its lexer only emits `&`/`|`
            // for the type grammar, so they must not bind as binops here.
            if op.is_bitwise() && !self.version.has_bitwise_ops() {
                break;
            }
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
            return Expression::UnaryOp(Box::new(UnaryOp { span, op, operand }));
        }
        let primary = self.parse_primary_expression();
        // Luau type assertions (`expr :: Type`) apply to ANY simpleexp, not just
        // prefix expressions, so they're handled here rather than inside the
        // prefix-only suffix loop. This lets `1 :: number`, `{} :: Foo`,
        // `f() :: T`, etc. parse - matching Luau's `asexp = simpleexp ['::' Type]`.
        self.parse_type_assertions(primary)
    }

    /// Wrap a just-parsed primary in a trailing Luau `:: Type` assertion.
    /// A no-op outside Luau. Exactly one cast per simpleexp, matching the
    /// grammar's `asexp ::= simpleexp ['::' Type]` - `x :: A :: B` is a
    /// parse error in real Luau, so a second `::` is left for the caller
    /// to reject as an unexpected token.
    fn parse_type_assertions(&mut self, expr: Expression) -> Expression {
        if self.version.is_luau() && matches!(self.peek(), TokenKind::DoubleColon) {
            self.advance_span();
            let type_annotation = self.parse_type();
            let span = expr.span().merge(type_annotation.span());
            return Expression::TypeCast(Box::new(TypeCast {
                span,
                expr,
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
            TokenKind::Number(_) => number_from(self.advance()),
            TokenKind::StringLiteral(_) => Expression::StringLiteral(literal_from(self.advance())),
            TokenKind::DotDotDot => {
                let span = self.advance_span();
                if !self.is_vararg_scope {
                    self.error(
                        span,
                        "cannot use '...' outside a vararg function".to_string(),
                    );
                }
                Expression::VarArg(span)
            }
            TokenKind::Function => self.parse_function_def(),
            // Luau: `simpleexp ::= attributes 'function' funcbody`
            TokenKind::At if self.version.is_luau() => self.parse_function_def(),
            TokenKind::LeftBrace => self.parse_table_constructor_expr(),
            TokenKind::LeftParen => {
                let open = self.advance_span();
                if let Err(err) = self.enter_depth() {
                    self.errors.push(err);
                    return Expression::Error(open);
                }
                let expr = self.parse_expression(0);
                self.exit_depth();
                let close = self.expect(&TokenKind::RightParen);
                let span = open.merge(close);
                let paren_expr =
                    Expression::Parenthesized(Box::new(ParenExpression { span, expr }));
                self.parse_suffixes(paren_expr)
            }
            // Luau if-expression: `if cond then expr {elseif cond then expr} else expr`
            TokenKind::If if self.version.is_luau() => self.parse_if_expression(),
            // Luau interpolated string
            TokenKind::InterpBegin(_) => self.parse_interpolated_string(),
            TokenKind::Identifier(_) => {
                let name_token = self.advance();
                let var_expr = Expression::Var(Var::Name(name_token));
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
                // Luau: explicit type-parameter instantiation is a suffix,
                // so it composes with field access, indexing, and calls.
                TokenKind::Less
                    if self.version.has_explicit_type_instantiation()
                        && matches!(self.peek_next(), TokenKind::Less) =>
                {
                    self.advance_span(); // first `<`
                    let type_args = self.parse_type_args(); // second `<` through first `>`
                    let close = self.expect(&TokenKind::Greater);
                    let span = expr.span().merge(close);
                    expr = Expression::TypeInstantiation(Box::new(TypeInstantiation {
                        span,
                        expr,
                        type_args,
                    }));
                }
                TokenKind::Dot => {
                    self.advance_span();
                    let name = self.expect_identifier_recover();
                    let span = expr.span().merge(name.span);
                    expr = Expression::Var(Var::FieldAccess(Box::new(FieldAccess {
                        span,
                        prefix: expr,
                        name,
                    })));
                }
                TokenKind::LeftBracket => {
                    self.advance_span();
                    let index = self.parse_expression(0);
                    let close = self.expect(&TokenKind::RightBracket);
                    let span = expr.span().merge(close);
                    expr = Expression::Var(Var::Index(Box::new(IndexExpression {
                        span,
                        prefix: expr,
                        index,
                    })));
                }
                TokenKind::Colon => {
                    self.advance_span();
                    let method_name = self.expect_identifier_recover();
                    let explicit_type_args = if self.version.has_explicit_type_instantiation()
                        && matches!(self.peek(), TokenKind::Less)
                        && matches!(self.peek_next(), TokenKind::Less)
                    {
                        self.advance_span(); // first `<`
                        let type_args = self.parse_type_args(); // second `<` through first `>`
                        self.expect(&TokenKind::Greater);
                        Some(Box::new(type_args))
                    } else {
                        None
                    };
                    let args = self.parse_function_args();
                    let args_span = function_args_span(&args);
                    let span = expr.span().merge(args_span);
                    expr = Expression::FunctionCall(Box::new(FunctionCall {
                        span,
                        callee: expr,
                        args,
                        method: Some(method_name),
                        explicit_type_args,
                    }));
                }
                TokenKind::LeftParen | TokenKind::LeftBrace | TokenKind::StringLiteral(_) => {
                    // 5.1 and Luau reject a call whose `(` starts on a new
                    // line ("ambiguous syntax: function call x new
                    // statement"); 5.2+ dropped the restriction.
                    if matches!(self.peek(), TokenKind::LeftParen)
                        && self.version.has_ambiguous_call_newline_error()
                        && self.newline_before_current()
                    {
                        let span = self.current_span();
                        self.error(
                            span,
                            "ambiguous syntax (function call x new statement): put '(' on the same line".to_string(),
                        );
                    }
                    let args = self.parse_function_args();
                    let args_span = function_args_span(&args);
                    let span = expr.span().merge(args_span);
                    expr = Expression::FunctionCall(Box::new(FunctionCall {
                        span,
                        callee: expr,
                        args,
                        method: None,
                        explicit_type_args: None,
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
        self.expect(&TokenKind::Then);
        let then_expr = self.parse_expression(0);

        let mut elseif_clauses = Vec::new();
        while matches!(self.peek(), TokenKind::ElseIf) {
            let elseif_token = self.advance_span();
            let elseif_condition = self.parse_expression(0);
            self.expect(&TokenKind::Then);
            let elseif_expr = self.parse_expression(0);
            let span = elseif_token.merge(elseif_expr.span());
            elseif_clauses.push(ElseIfExprClause {
                span,
                condition: elseif_condition,
                expr: elseif_expr,
            });
        }

        self.expect(&TokenKind::Else);
        let else_expr = self.parse_expression(0);
        self.exit_depth();
        let span = if_token.merge(else_expr.span());

        Expression::IfExpression(Box::new(IfExpression {
            span,
            condition,
            then_expr,
            elseif_clauses,
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
                // A queued plain-string InterpEnd points back INSIDE the
                // Begin token's span; a detached InterpEnd means `{}`
                // held no expression, which real Luau rejects.
                if end_token.span.start >= begin_token.span.end {
                    self.error(
                        end_token.span,
                        "malformed interpolated string: expected expression inside '{}'"
                            .to_string(),
                    );
                }
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
                let close = self.expect(&TokenKind::RightParen);
                FunctionArgs::Parenthesized {
                    span: open.merge(close),
                    args,
                }
            }
            TokenKind::LeftBrace => {
                let table = self.parse_table_constructor();
                FunctionArgs::TableConstructor(Box::new(table))
            }
            TokenKind::StringLiteral(_) => {
                FunctionArgs::StringLiteral(literal_from(self.advance()))
            }
            _ => {
                let span = self.current_span();
                self.error(span, "expected function arguments".to_string());
                FunctionArgs::Parenthesized {
                    span,
                    args: Punctuated::empty(),
                }
            }
        }
    }

    /// Parse `function(params) block end`, optionally preceded by Luau
    /// `@attr` attributes (`simpleexp ::= attributes 'function' funcbody`).
    fn parse_function_def(&mut self) -> Expression {
        let attributes = if matches!(self.peek(), TokenKind::At) {
            self.parse_function_attributes()
        } else {
            Vec::new()
        };
        let start_span = attributes
            .first()
            .map(|attr| attr.span)
            .unwrap_or(self.current_span());
        if !matches!(self.peek(), TokenKind::Function) {
            let span = self.current_span();
            self.error(span, "expected 'function' after attribute".to_string());
            return Expression::Error(span);
        }
        self.advance_span(); // `function`
        let body = self.parse_function_body();
        let span = start_span.merge(body.span);
        Expression::FunctionDef(Box::new(FunctionDef {
            span,
            attributes,
            body,
        }))
    }

    /// Parse function body: `[<generics>] (params) block end`.
    pub(crate) fn parse_function_body(&mut self) -> FunctionBody {
        // Luau: generic list before the parens - `function f<T>(x: T)`
        let generics = if self.version.is_luau() && matches!(self.peek(), TokenKind::Less) {
            Some(Box::new(self.parse_generic_type_list(false)))
        } else {
            None
        };

        let open = self.expect(&TokenKind::LeftParen);
        let start_span = generics.as_ref().map(|list| list.span).unwrap_or(open);

        if let Err(err) = self.enter_depth() {
            self.errors.push(err);
            return FunctionBody {
                span: start_span,
                generics,
                params: Punctuated::empty(),
                vararg: None,
                return_type: None,
                block: luck_ast::Block {
                    span: start_span,
                    stmts: Vec::new(),
                    last_stmt: None,
                },
            };
        }

        let mut params = Punctuated::<Parameter>::empty();
        let mut vararg = None;

        if !matches!(self.peek(), TokenKind::RightParen) {
            if matches!(self.peek(), TokenKind::DotDotDot) {
                vararg = Some(self.parse_vararg_param());
            } else if self.check_identifier() {
                let first_name = self.advance();
                let type_ann = self.try_parse_type_annotation();
                params.push(Parameter {
                    span: first_name.span.merge(
                        type_ann
                            .as_ref()
                            .map(|annotation| annotation.span())
                            .unwrap_or(first_name.span),
                    ),
                    name: first_name,
                    type_annotation: type_ann,
                });

                while matches!(self.peek(), TokenKind::Comma) {
                    self.advance_span();
                    if matches!(self.peek(), TokenKind::DotDotDot) {
                        // Vararg after last named param
                        vararg = Some(self.parse_vararg_param());
                        break;
                    }
                    let name = self.expect_identifier_recover();
                    let type_ann = self.try_parse_type_annotation();
                    params.push(Parameter {
                        span: name.span.merge(
                            type_ann
                                .as_ref()
                                .map(|annotation| annotation.span())
                                .unwrap_or(name.span),
                        ),
                        name,
                        type_annotation: type_ann,
                    });
                }
            }
        }

        self.expect(&TokenKind::RightParen);

        // Luau: optional return type annotation after `)`
        let return_type = self.try_parse_type_annotation();

        // A function body is a fresh control-flow and vararg scope:
        // break/continue may not escape into it, and `...` is only
        // valid when this function's params end in `...`.
        let saved_loop_depth = std::mem::replace(&mut self.loop_depth, 0);
        let saved_vararg_scope = std::mem::replace(&mut self.is_vararg_scope, vararg.is_some());
        self.function_depth += 1;
        let block = self.parse_block();
        self.function_depth -= 1;
        self.loop_depth = saved_loop_depth;
        self.is_vararg_scope = saved_vararg_scope;

        let end_token = self.expect(&TokenKind::End);

        self.exit_depth();

        let span = start_span.merge(end_token);

        FunctionBody {
            span,
            generics,
            params,
            vararg,
            return_type,
            block,
        }
    }

    /// Parse `... [name] [: type]` (assumes `...` is the current token).
    fn parse_vararg_param(&mut self) -> VarArgParam {
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
            .map(|annotation| annotation.span())
            .or(vararg_name.as_ref().map(|n| n.span))
            .unwrap_or(dots);
        VarArgParam {
            span: dots.merge(end_span),
            name: vararg_name,
            type_annotation: vararg_type,
        }
    }

    /// Parse a table constructor: `{ [fieldlist] }`.
    pub(crate) fn parse_table_constructor(&mut self) -> TableConstructor {
        let start_span = self.advance_span(); // `{`

        if let Err(err) = self.enter_depth() {
            self.errors.push(err);
            return TableConstructor {
                span: start_span,
                fields: Punctuated::empty(),
            };
        }

        let mut fields = Punctuated::empty();

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

            fields.push(self.parse_field());
            let has_separator = matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon);
            if has_separator {
                self.advance_span();
            }
            fields.has_trailing_separator = has_separator;

            if !has_separator {
                break;
            }
        }

        self.exit_depth();

        let close = self.expect(&TokenKind::RightBrace);
        let span = start_span.merge(close);

        TableConstructor { span, fields }
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
            self.expect(&TokenKind::RightBracket);
            self.expect(&TokenKind::Equal);
            let value = self.parse_expression(0);
            let span = open.merge(value.span());
            return Field::Bracketed { span, key, value };
        }

        // `Name = expr` - need lookahead: identifier followed by `=`
        if self.check_identifier() && matches!(self.peek_next(), TokenKind::Equal) {
            let name = self.advance();
            self.advance_span();
            let value = self.parse_expression(0);
            let span = name.span.merge(value.span());
            return Field::Named { span, name, value };
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
        FunctionArgs::Parenthesized { span, .. } => *span,
        FunctionArgs::TableConstructor(t) => t.span,
        FunctionArgs::StringLiteral(t) => t.span,
    }
}
