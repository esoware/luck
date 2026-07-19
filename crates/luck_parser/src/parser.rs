use luck_ast::{Block, shared::Punctuated};
use luck_lexer::LexError;
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

/// Recursive descent parser that converts a token stream into a Lua AST.
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    pub comments: Vec<Comment>,
    pub version: LuaVersion,
    depth: u32,
    max_depth: u32,
    pub(crate) errors: Vec<ParseError>,
    /// Span covering the entire source, used for EOF positions
    source_len: u32,
    /// Stack tracking what construct is being parsed, for contextual error messages.
    pub(crate) context_stack: Vec<(&'static str, Span)>,
}

/// Sentinel returned when the token stream is exhausted.
const EOF_KIND: TokenKind = TokenKind::Eof;

impl Parser {
    pub fn new(
        tokens: Vec<Token>,
        comments: Vec<Comment>,
        version: LuaVersion,
        source: &str,
    ) -> Self {
        Self {
            tokens,
            pos: 0,
            comments,
            version,
            depth: 0,
            max_depth: 256,
            errors: Vec::new(),
            source_len: source.len() as u32,
            context_stack: Vec::new(),
        }
    }

    #[inline]
    pub fn peek(&self) -> &TokenKind {
        self.tokens
            .get(self.pos)
            .map(|t| &t.kind)
            .unwrap_or(&EOF_KIND)
    }

    #[inline]
    pub fn peek_token(&self) -> &Token {
        static EOF_TOKEN: std::sync::LazyLock<Token> =
            std::sync::LazyLock::new(|| Token::new(TokenKind::Eof, Span::new(0, 0)));
        self.tokens.get(self.pos).unwrap_or(&EOF_TOKEN)
    }

    /// Look ahead by `offset` tokens (0 = current).
    #[inline]
    pub fn peek_at(&self, offset: usize) -> &TokenKind {
        self.tokens
            .get(self.pos + offset)
            .map(|t| &t.kind)
            .unwrap_or(&EOF_KIND)
    }

    #[inline]
    pub fn advance(&mut self) -> Token {
        if self.pos < self.tokens.len() {
            // Tokens are consumed exactly once (pos never rewinds), so
            // take ownership instead of deep-cloning every CompactString
            // payload. The placeholder keeps the original span because
            // `tokens[pos - 1].span` is read after consumption for
            // previous-token diagnostics.
            let span = self.tokens[self.pos].span;
            let token =
                std::mem::replace(&mut self.tokens[self.pos], Token::new(TokenKind::Eof, span));
            self.pos += 1;
            token
        } else {
            Token::new(TokenKind::Eof, self.eof_span())
        }
    }

    pub fn check_identifier(&self) -> bool {
        matches!(self.peek(), TokenKind::Identifier(_))
    }

    pub fn expect(&mut self, kind: &TokenKind) -> Result<Token, ParseError> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(kind) {
            Ok(self.advance())
        } else {
            let span = self.current_span();
            let message = format!("expected {}, found {}", kind, self.peek());
            Err(Self::make_error(span, message))
        }
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

    /// Merge lex errors and parse errors, sorted by position.
    pub fn into_errors(mut self, lex_errors: Vec<LexError>) -> Vec<ParseError> {
        let mut parse_errors: Vec<ParseError> = lex_errors
            .into_iter()
            .map(|e| ParseError {
                span: e.span,
                message: e.message,
            })
            .collect();
        parse_errors.append(&mut self.errors);
        parse_errors.sort_by_key(|e| e.span.start);
        parse_errors
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
                    let token = self.advance();
                    stmts.push(luck_ast::Statement::EmptyStatement(token));
                } else {
                    // 5.1: just consume semicolons as separators
                    self.advance();
                }
            }

            match self.peek() {
                TokenKind::Return => {
                    last_stmt = Some(Box::new(self.parse_return_statement()));
                    break;
                }
                TokenKind::Break if self.version.break_is_last_stat_only() => {
                    let token = self.advance();
                    last_stmt = Some(Box::new(luck_ast::LastStatement::Break(token)));
                    if matches!(self.peek(), TokenKind::Semicolon) {
                        self.advance();
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
                        && is_continue_context(self.peek_at(1)) =>
                {
                    let token = self.advance();
                    last_stmt = Some(Box::new(luck_ast::LastStatement::Continue(token)));
                    if matches!(self.peek(), TokenKind::Semicolon) {
                        self.advance();
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
        self.peek_token().span
    }

    fn previous_span(&self) -> Span {
        if self.pos > 0 {
            self.tokens[self.pos - 1].span
        } else {
            Span::new(0, 0)
        }
    }

    pub fn eof_span(&self) -> Span {
        Span::new(self.source_len, self.source_len)
    }

    /// Consume a closing `>` in type context, recovering if absent.
    /// Adjacent tokens lex greedily - `Foo<Bar<T>>` produces `ShiftRight`,
    /// `Foo<T>=x` produces `GreaterEqual` - so those are split: the first
    /// `>` is returned and the remainder stays current.
    pub(crate) fn consume_type_close_angle(&mut self) -> Token {
        match self.peek() {
            TokenKind::Greater => self.advance(),
            TokenKind::ShiftRight => {
                let span = self.tokens[self.pos].span;
                self.tokens[self.pos] =
                    Token::new(TokenKind::Greater, Span::new(span.start + 1, span.end));
                Token::new(TokenKind::Greater, Span::new(span.start, span.start + 1))
            }
            TokenKind::GreaterEqual => {
                let span = self.tokens[self.pos].span;
                self.tokens[self.pos] =
                    Token::new(TokenKind::Equal, Span::new(span.start + 1, span.end));
                Token::new(TokenKind::Greater, Span::new(span.start, span.start + 1))
            }
            _ => {
                let span = self.current_span();
                let message = format!("expected > to close generics, found {}", self.peek());
                self.error(span, message);
                Token::new(TokenKind::Greater, span)
            }
        }
    }

    /// Parse a comma-separated list of expressions.
    pub fn parse_expression_list(&mut self) -> Punctuated<luck_ast::Expression> {
        let mut pairs = Vec::new();
        let mut current = self.parse_expression(0);

        while matches!(self.peek(), TokenKind::Comma) {
            let comma = self.advance();
            let next = self.parse_expression(0);
            pairs.push((current, comma));
            current = next;
        }

        Punctuated::from_pairs(pairs, Some(current))
    }
}
