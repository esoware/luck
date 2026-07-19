use luck_ast::{Block, shared::Punctuated};
use luck_lexer::Lexer;
use luck_token::{Comment, LuaVersion, Span, Token, TokenKind};

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
pub struct Parser<'src> {
    lexer: Lexer<'src>,
    current: Token,
    next: Token,
    prev_span: Span,
    pub version: LuaVersion,
    depth: u32,
    max_depth: u32,
    pub(crate) errors: Vec<ParseError>,
    /// Stack tracking what construct is being parsed, for contextual error messages.
    pub(crate) context_stack: Vec<(&'static str, Span)>,
}

impl<'src> Parser<'src> {
    pub fn new(source: &'src str, version: LuaVersion) -> Self {
        let mut lexer = Lexer::new(source, version);
        let current = lexer.next_token();
        let next = lexer.next_token();
        Self {
            lexer,
            current,
            next,
            prev_span: Span::new(0, 0),
            version,
            depth: 0,
            max_depth: 256,
            errors: Vec::new(),
            context_stack: Vec::new(),
        }
    }

    #[inline]
    pub fn peek(&self) -> &TokenKind {
        &self.current.kind
    }

    /// One token of lookahead - all the Lua grammar ever needs.
    #[inline]
    pub fn peek_next(&self) -> &TokenKind {
        &self.next.kind
    }

    #[inline]
    pub fn advance(&mut self) -> Token {
        self.prev_span = self.current.span;
        std::mem::replace(
            &mut self.current,
            std::mem::replace(&mut self.next, self.lexer.next_token()),
        )
    }

    /// Consume the current token, keeping only its span.
    #[inline]
    pub fn advance_span(&mut self) -> Span {
        self.advance().span
    }

    /// `expect` for fixed-spelling tokens: returns only the span.
    pub fn expect_span(&mut self, kind: &TokenKind) -> Result<Span, ParseError> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(kind) {
            Ok(self.advance_span())
        } else {
            let span = self.current_span();
            let message = format!("expected {}, found {}", kind, self.peek());
            Err(Self::make_error(span, message))
        }
    }

    pub fn check_identifier(&self) -> bool {
        matches!(self.peek(), TokenKind::Identifier(_))
    }

    pub fn expect_identifier(&mut self) -> Result<Token, ParseError> {
        if self.check_identifier() {
            Ok(self.advance())
        } else {
            let span = self.current_span();
            let message = format!("expected identifier, found {}", self.peek());
            Err(Self::make_error(span, message))
        }
    }

    #[inline]
    pub fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    #[inline]
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn enter_depth(&mut self) -> Result<(), ParseError> {
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

    pub fn exit_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Push a parsing context for better error messages (e.g. "in if-statement").
    #[inline]
    pub fn push_context(&mut self, context: &'static str, span: Span) {
        self.context_stack.push((context, span));
    }

    #[inline]
    pub fn pop_context(&mut self) {
        self.context_stack.pop();
    }

    #[cold]
    pub fn error(&mut self, span: Span, message: String) {
        self.errors.push(ParseError { span, message });
    }

    #[cold]
    fn make_error(span: Span, message: String) -> ParseError {
        ParseError { span, message }
    }

    /// Skip tokens until we reach a statement boundary for error recovery.
    /// Always consumes at least one token to prevent infinite loops.
    pub fn synchronize(&mut self) {
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
    pub fn finish(mut self) -> (Vec<Comment>, Vec<ParseError>) {
        while !matches!(self.lexer.next_token().kind, TokenKind::Eof) {}
        let (comments, lex_errors) = self.lexer.finish();
        let mut errors = lex_errors;
        errors.append(&mut self.errors);
        errors.sort_by_key(|e| e.span.start);
        (comments, errors)
    }

    /// Parse a block (statement list with optional trailing return/break/continue).
    pub fn parse_block(&mut self) -> Block {
        let start_span = self.current_span();

        if let Err(err) = self.enter_depth() {
            self.errors.push(err);
            return Block {
                span: start_span,
                stmts: Vec::new(),
                last_stmt: None,
            };
        }

        let mut stmts = Vec::new();
        let mut last_stmt = None;

        loop {
            // Skip semicolons in Lua 5.1 (they are statement separators, not statements)
            while matches!(self.peek(), TokenKind::Semicolon) {
                if self.version.has_empty_statement() {
                    // 5.2+: semicolons are empty statements
                    let span = self.advance_span();
                    stmts.push(luck_ast::Statement::EmptyStatement(span));
                } else {
                    // 5.1: just consume semicolons as separators
                    self.advance_span();
                }
            }

            match self.peek() {
                TokenKind::Return => {
                    last_stmt = Some(Box::new(self.parse_return_statement()));
                    break;
                }
                TokenKind::Break if self.version.break_is_last_stat_only() => {
                    let span = self.advance_span();
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
                    last_stmt = Some(Box::new(luck_ast::LastStatement::Continue(span)));
                    if matches!(self.peek(), TokenKind::Semicolon) {
                        self.advance_span();
                    }
                    break;
                }
                kind if kind.is_stat_start() => {
                    match self.parse_statement() {
                        Some(stmt) => stmts.push(stmt),
                        None => {
                            // parse_statement returned None - error recovery already happened
                        }
                    }
                }
                _ => {
                    // Unknown token that doesn't start a statement and isn't a block-ender.
                    // Error-recover: record the error, synchronize, and keep parsing.
                    let span = self.current_span();
                    self.error(span, format!("unexpected token {}", self.peek()));
                    self.synchronize();
                    stmts.push(luck_ast::Statement::Error(span));
                }
            }
        }

        self.exit_depth();

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
    pub fn current_span(&self) -> Span {
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
    pub fn parse_expression_list(&mut self) -> Punctuated<luck_ast::Expression> {
        let mut pairs = Vec::new();
        let mut current = self.parse_expression(0);

        while matches!(self.peek(), TokenKind::Comma) {
            let comma = self.advance_span();
            let next = self.parse_expression(0);
            pairs.push((current, comma));
            current = next;
        }

        Punctuated::from_pairs(pairs, Some(current))
    }
}
