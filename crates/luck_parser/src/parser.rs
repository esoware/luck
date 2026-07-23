use luck_ast::{Block, shared::Punctuated};
use luck_lexer::Lexer;
use luck_token::{Comment, LuaVersion, Span, Token, TokenKind};
use std::collections::HashSet;

use crate::ParseError;

/// Whether the next token after `continue` confirms it's a continue statement
/// (not a function call like `continue()` or assignment like `continue = 5`).
fn is_continue_context(next: &TokenKind) -> bool {
    matches!(
        next,
        TokenKind::End
            | TokenKind::Else
            | TokenKind::ElseIf
            | TokenKind::Until
            | TokenKind::Eof
            | TokenKind::Semicolon
    )
}

/// Recursive descent parser that consumes tokens straight from the lexer.
/// Lua needs exactly one token of lookahead, so the parser holds a
/// two-slot `current`/`next` buffer instead of a materialized token
/// stream.
pub(crate) struct Parser<'src> {
    lexer: Lexer<'src>,
    source: &'src str,
    current: Token,
    next: Token,
    prev_span: Span,
    pub(crate) version: LuaVersion,
    depth: u32,
    max_depth: u32,
    /// Lexical block depth. The root module block is 1.
    pub(crate) block_depth: u32,
    /// Function nesting depth, used to distinguish module returns.
    pub(crate) function_depth: u32,
    pub(crate) has_module_return: bool,
    pub(crate) has_value_exports: bool,
    pub(crate) exported_names: HashSet<luck_token::CompactString>,
    /// Nesting depth of enclosing loops; reset inside function bodies.
    /// `break`/`continue` at depth 0 is a compile error in every Lua.
    pub(crate) loop_depth: u32,
    /// Whether `...` is valid here. Main chunks are vararg; function
    /// bodies are vararg only when their params end in `...`.
    pub(crate) is_vararg_scope: bool,
    pub(crate) errors: Vec<ParseError>,
    /// Stack tracking what construct is being parsed, for contextual error messages.
    pub(crate) context_stack: Vec<(&'static str, Span)>,
}

impl<'src> Parser<'src> {
    pub(crate) fn new(source: &'src str, version: LuaVersion) -> Self {
        let mut lexer = Lexer::new(source, version);
        let current = lexer.next_token();
        let next = lexer.next_token();
        Self {
            lexer,
            source,
            current,
            next,
            prev_span: Span::new(0, 0),
            version,
            depth: 0,
            max_depth: 256,
            block_depth: 0,
            function_depth: 0,
            has_module_return: false,
            has_value_exports: false,
            exported_names: HashSet::new(),
            loop_depth: 0,
            is_vararg_scope: true,
            errors: Vec::new(),
            context_stack: Vec::new(),
        }
    }

    /// Whether the source between the previous token and the current one
    /// contains a line break. Drives the 5.1/Luau "ambiguous syntax"
    /// check for `(` call suffixes.
    pub(crate) fn newline_before_current(&self) -> bool {
        let between = &self.source[self.prev_span.end as usize..self.current.span.start as usize];
        between.bytes().any(|b| b == b'\n' || b == b'\r')
    }

    #[inline]
    pub(crate) fn peek(&self) -> &TokenKind {
        &self.current.kind
    }

    /// One token of lookahead - all the Lua grammar ever needs.
    #[inline]
    pub(crate) fn peek_next(&self) -> &TokenKind {
        &self.next.kind
    }

    #[inline]
    pub(crate) fn advance(&mut self) -> Token {
        self.prev_span = self.current.span;
        std::mem::replace(
            &mut self.current,
            std::mem::replace(&mut self.next, self.lexer.next_token()),
        )
    }

    /// Consume the current token, keeping only its span.
    #[inline]
    pub(crate) fn advance_span(&mut self) -> Span {
        self.advance().span
    }

    /// `expect` for fixed-spelling tokens: returns only the span.
    #[inline]
    pub(crate) fn expect_span(&mut self, kind: &TokenKind) -> Result<Span, ParseError> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(kind) {
            Ok(self.advance_span())
        } else {
            let span = self.current_span();
            let message = format!("expected {}, found {}", kind, self.peek());
            Err(Self::make_error(span, message))
        }
    }

    pub(crate) fn check_identifier(&self) -> bool {
        matches!(self.peek(), TokenKind::Identifier(_))
    }

    #[inline]
    pub(crate) fn expect_identifier(&mut self) -> Result<Token, ParseError> {
        if self.check_identifier() {
            Ok(self.advance())
        } else {
            let span = self.current_span();
            let message = format!("expected identifier, found {}", self.peek());
            Err(Self::make_error(span, message))
        }
    }

    /// Expect `kind`; on mismatch record the default "expected X, found Y"
    /// error and return the current span so parsing continues past the
    /// missing token. The recovering counterpart to [`Self::expect_span`].
    #[inline]
    pub(crate) fn expect(&mut self, kind: &TokenKind) -> Span {
        self.expect_span(kind).unwrap_or_else(|err| {
            self.errors.push(err);
            self.current_span()
        })
    }

    /// The one canonical zero-width identifier placeholder, anchored at
    /// `span`, that keeps parsing alive after a failed identifier
    /// expectation or an invalid assignment target.
    pub(crate) fn recovery_identifier(span: Span) -> Token {
        Token::new(TokenKind::Identifier(String::new().into()), span)
    }

    /// Expect an identifier; on mismatch record the error and return a
    /// zero-width placeholder token so parsing continues.
    #[inline]
    pub(crate) fn expect_identifier_recover(&mut self) -> Token {
        self.expect_identifier().unwrap_or_else(|err| {
            self.errors.push(err);
            Self::recovery_identifier(self.current_span())
        })
    }

    #[inline]
    pub(crate) fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    #[inline]
    pub(crate) fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub(crate) fn enter_depth(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > self.max_depth {
            let span = self.current_span();
            Err(Self::make_error(
                span,
                "maximum nesting depth exceeded".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn exit_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Push a parsing context for better error messages (e.g. "in if-statement").
    #[inline]
    pub(crate) fn push_context(&mut self, context: &'static str, span: Span) {
        self.context_stack.push((context, span));
    }

    #[inline]
    pub(crate) fn pop_context(&mut self) {
        self.context_stack.pop();
    }

    #[cold]
    pub(crate) fn error(&mut self, span: Span, message: String) {
        self.errors.push(ParseError { span, message });
    }

    #[cold]
    fn make_error(span: Span, message: String) -> ParseError {
        ParseError { span, message }
    }

    /// Skip tokens until we reach a statement boundary for error recovery.
    /// Always consumes at least one token to prevent infinite loops.
    pub(crate) fn synchronize(&mut self) {
        self.advance();
        while !self.at_eof() {
            if self.peek().is_stat_start() {
                return;
            }
            match self.peek() {
                TokenKind::End
                | TokenKind::Else
                | TokenKind::ElseIf
                | TokenKind::Until
                | TokenKind::RightParen
                | TokenKind::RightBrace
                | TokenKind::RightBracket => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    /// Drain the lexer to EOF (comments and lex errors past the last
    /// parsed statement must still be collected), then yield the
    /// comments and all errors merged and sorted by position.
    pub(crate) fn finish(mut self) -> (Vec<Comment>, Vec<ParseError>) {
        while !matches!(self.lexer.next_token().kind, TokenKind::Eof) {}
        let (comments, lex_errors) = self.lexer.finish();
        let mut errors = lex_errors;
        errors.append(&mut self.errors);
        errors.sort_by_key(|e| e.span.start);
        (comments, errors)
    }

    /// Parse a loop body: identical to `parse_block` but with
    /// `break`/`continue` made valid inside it.
    pub(crate) fn parse_loop_block(&mut self) -> Block {
        self.loop_depth += 1;
        let block = self.parse_block();
        self.loop_depth -= 1;
        block
    }

    /// Parse a block (statement list with optional trailing return/break/continue).
    pub(crate) fn parse_block(&mut self) -> Block {
        let start_span = self.current_span();
        self.block_depth += 1;

        if let Err(err) = self.enter_depth() {
            self.errors.push(err);
            self.block_depth = self.block_depth.saturating_sub(1);
            return Block {
                span: start_span,
                stmts: Vec::new(),
                last_stmt: None,
            };
        }

        let mut stmts = Vec::new();
        let mut last_stmt = None;
        // 5.1 and Luau have no empty statement: a single `;` may only
        // FOLLOW a statement (block ::= {stat [';']}), so a leading or
        // doubled `;` is a parse error there. 5.2+ treats `;` as a
        // statement of its own.
        let mut can_take_separator = false;

        loop {
            while matches!(self.peek(), TokenKind::Semicolon) {
                if self.version.has_empty_statement() {
                    let span = self.advance_span();
                    stmts.push(luck_ast::Statement::EmptyStatement(span));
                } else {
                    let span = self.current_span();
                    if !can_take_separator {
                        self.error(span, "unexpected token ';'".to_string());
                    }
                    self.advance_span();
                    can_take_separator = false;
                }
            }

            match self.peek() {
                TokenKind::Return => {
                    last_stmt = Some(Box::new(self.parse_return_statement()));
                    break;
                }
                TokenKind::Break if self.version.break_is_last_stat_only() => {
                    let span = self.advance_span();
                    if self.loop_depth == 0 {
                        self.error(span, "break outside a loop".to_string());
                    }
                    last_stmt = Some(Box::new(luck_ast::LastStatement::Break(span)));
                    if matches!(self.peek(), TokenKind::Semicolon) {
                        self.advance_span();
                    }
                    break;
                }
                // Block-ending tokens: don't consume, let caller handle
                TokenKind::End
                | TokenKind::Else
                | TokenKind::ElseIf
                | TokenKind::Until
                | TokenKind::Eof => break,
                // Luau `continue` as last statement (context-sensitive identifier)
                TokenKind::Identifier(name)
                    if self.version.is_luau()
                        && name == "continue"
                        && is_continue_context(self.peek_next()) =>
                {
                    let span = self.advance_span();
                    if self.loop_depth == 0 {
                        self.error(span, "continue outside a loop".to_string());
                    }
                    last_stmt = Some(Box::new(luck_ast::LastStatement::Continue(span)));
                    if matches!(self.peek(), TokenKind::Semicolon) {
                        self.advance_span();
                    }
                    break;
                }
                kind if kind.is_stat_start() => {
                    stmts.push(self.parse_statement());
                    can_take_separator = true;
                }
                _ => {
                    // Unknown token that doesn't start a statement and isn't a block-ender.
                    // Error-recover: record the error, synchronize, and keep parsing.
                    let span = self.current_span();
                    self.error(span, format!("unexpected token {}", self.peek()));
                    self.synchronize();
                    stmts.push(luck_ast::Statement::Error(span));
                    can_take_separator = true;
                }
            }
        }

        self.exit_depth();
        self.block_depth = self.block_depth.saturating_sub(1);

        let end_span = if stmts.is_empty() && last_stmt.is_none() {
            start_span
        } else {
            self.previous_span()
        };
        let span = start_span.merge(end_span);

        Block {
            span,
            stmts,
            last_stmt,
        }
    }

    #[inline]
    pub(crate) fn current_span(&self) -> Span {
        self.current.span
    }

    fn previous_span(&self) -> Span {
        self.prev_span
    }

    /// Consume a closing `>` in type context, recovering if absent.
    /// Adjacent tokens lex greedily - `Foo<Bar<T>>` produces `ShiftRight`,
    /// `Foo<T>=x` produces `GreaterEqual` - so those are split: the first
    /// `>`'s span is returned and the remainder stays current.
    pub(crate) fn consume_type_close_angle(&mut self) -> Span {
        match self.peek() {
            TokenKind::Greater => self.advance_span(),
            TokenKind::ShiftRight => {
                let span = self.current.span;
                self.current = Token::new(TokenKind::Greater, Span::new(span.start + 1, span.end));
                Span::new(span.start, span.start + 1)
            }
            TokenKind::GreaterEqual => {
                let span = self.current.span;
                self.current = Token::new(TokenKind::Equal, Span::new(span.start + 1, span.end));
                Span::new(span.start, span.start + 1)
            }
            _ => {
                let span = self.current_span();
                let message = format!("expected > to close generics, found {}", self.peek());
                self.error(span, message);
                span
            }
        }
    }

    /// Parse a comma-separated list of expressions.
    pub(crate) fn parse_expression_list(&mut self) -> Punctuated<luck_ast::Expression> {
        let mut exprs = vec![self.parse_expression(0)];

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance_span();
            exprs.push(self.parse_expression(0));
        }

        Punctuated::from_items(exprs)
    }
}
