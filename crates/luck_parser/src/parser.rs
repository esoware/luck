mod attributes;
mod expr;
mod stmt;
mod types;

use core::mem;
use core::ops::Range;

use luck_ast::Block;
use luck_lexer::Lexer;
use luck_token::{Comment, CompactString, LuaVersion, Span, Token, TokenKind};
use rustc_hash::FxHashSet;

const MAX_NESTING_DEPTH: u32 = 256;

/// The AST, comments, and errors from one parse, plus the parsed source.
#[derive(Debug)]
pub struct ParseResult {
    pub block: Block,
    pub comments: Vec<Comment>,
    pub errors: Vec<ParseError>,
    pub source: String,
}

pub type ParseError = luck_token::SourceError;

/// Parse `source` as a chunk of `version`.
///
/// An owned `String` moves into `ParseResult::source` without a copy; a
/// `&str` is copied once.
#[must_use]
pub fn parse<Source>(source: Source, version: LuaVersion) -> ParseResult
where
    Source: Into<String>,
{
    parse_owned(source.into(), version)
}

fn parse_owned(source: String, version: LuaVersion) -> ParseResult {
    let (block, comments, errors) = match Parser::new(&source, version) {
        Ok(parser) => parser.parse_chunk(),
        Err(error) => (
            Block { span: Span::new(0, 0), stmts: Vec::new(), last_stmt: None },
            Vec::new(),
            vec![error],
        ),
    };
    ParseResult { block, comments, errors, source }
}

/// Recursive descent parser that pulls tokens straight from the lexer.
///
/// Lua needs only one token of lookahead, so `current` and `next` stand in
/// for a materialized token stream.
pub(crate) struct Parser<'source> {
    lexer: Lexer<'source>,
    source: &'source str,
    current: Token,
    next: Token,
    previous_span: Span,
    version: LuaVersion,
    nesting_depth: u32,
    /// Lexical block depth. The chunk's own block is 1.
    block_depth: u32,
    /// Function nesting depth, used to distinguish module returns.
    function_depth: u32,
    has_module_return: bool,
    has_value_exports: bool,
    exported_names: FxHashSet<CompactString>,
    /// Number of enclosing loops, reset inside function bodies. `break` or
    /// `continue` at 0 is a compile error in every Lua.
    loop_depth: u32,
    /// Whether `...` is valid here. Main chunks are vararg; function
    /// bodies are vararg only when their params end in `...`.
    is_vararg_scope: bool,
    errors: Vec<ParseError>,
}

impl<'source> Parser<'source> {
    /// # Errors
    ///
    /// Returns the lexer's refusal of a source too large for `u32` spans.
    pub(crate) fn new(source: &'source str, version: LuaVersion) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(source, version)?;
        let current = lexer.next_token();
        let next = lexer.next_token();
        Ok(Self {
            lexer,
            source,
            current,
            next,
            previous_span: Span::new(0, 0),
            version,
            nesting_depth: 0,
            block_depth: 0,
            function_depth: 0,
            has_module_return: false,
            has_value_exports: false,
            exported_names: FxHashSet::default(),
            loop_depth: 0,
            is_vararg_scope: true,
            errors: Vec::new(),
        })
    }

    /// Parse the whole source as one chunk. Returns its block, every comment,
    /// and every lex and parse error sorted by position.
    pub(crate) fn parse_chunk(mut self) -> (Block, Vec<Comment>, Vec<ParseError>) {
        let block = self.parse_block();
        // `parse_block` stops at a block boundary such as `return` or a stray
        // `end`, and anything after it would silently drop out of the AST. After
        // an earlier error, recovery may already have abandoned the tail.
        if !self.is_at_eof() && self.errors.is_empty() {
            let span = self.current_span();
            let message = format!("expected end of file, found {}", self.peek());
            self.error(span, message);
        }
        // Drain the lexer so comments and lex errors past the last parsed
        // statement are kept.
        while !matches!(self.lexer.next_token().kind, TokenKind::Eof) {}
        let (comments, mut errors) = self.lexer.finish();
        errors.append(&mut self.errors);
        errors.sort_by_key(|error| error.span.start);
        (block, comments, errors)
    }

    /// Whether the source between the previous token and the current one
    /// contains a line break. Drives the 5.1/Luau "ambiguous syntax"
    /// check for `(` call suffixes.
    fn has_newline_before_current(&self) -> bool {
        let between = Span::new(self.previous_span.end, self.current.span.start);
        self.source
            .get(Range::<usize>::from(between))
            .is_some_and(|text| text.bytes().any(|byte| byte == b'\n' || byte == b'\r'))
    }

    #[inline]
    const fn peek(&self) -> &TokenKind {
        &self.current.kind
    }

    #[inline]
    const fn peek_next(&self) -> &TokenKind {
        &self.next.kind
    }

    #[inline]
    const fn current_span(&self) -> Span {
        self.current.span
    }

    #[inline]
    const fn is_at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    const fn is_at_identifier(&self) -> bool {
        matches!(self.peek(), TokenKind::Identifier(_))
    }

    /// Whether the current token is a `kind`, whatever its payload.
    #[inline]
    fn is_at(&self, kind: &TokenKind) -> bool {
        mem::discriminant(self.peek()) == mem::discriminant(kind)
    }

    #[inline]
    fn advance(&mut self) -> Token {
        self.previous_span = self.current.span;
        mem::replace(&mut self.current, mem::replace(&mut self.next, self.lexer.next_token()))
    }

    #[inline]
    fn advance_span(&mut self) -> Span {
        self.advance().span
    }

    /// Consume a token of `kind` and return its span. On a mismatch, record
    /// an "expected X, found Y" error, leave the token in place, and return
    /// its span so parsing can go on.
    #[inline]
    fn expect(&mut self, kind: &TokenKind) -> Span {
        if self.is_at(kind) { self.advance_span() } else { self.report_expected(kind) }
    }

    /// Consume the current token if it's an identifier. Otherwise record an
    /// "expected identifier" error and leave the token in place.
    #[inline]
    fn expect_identifier(&mut self) -> Option<Token> {
        if self.is_at_identifier() {
            Some(self.advance())
        } else {
            self.report_expected_identifier();
            None
        }
    }

    /// Like [`Self::expect_identifier`], but a missing name becomes
    /// [`Self::recovery_identifier`] at the current token.
    #[inline]
    fn expect_identifier_recover(&mut self) -> Token {
        self.expect_identifier().unwrap_or_else(|| Self::recovery_identifier(self.current_span()))
    }

    /// An identifier with an empty name at `span`, standing in for a missing
    /// name or an invalid assignment target so parsing can go on.
    fn recovery_identifier(span: Span) -> Token {
        Token::new(TokenKind::Identifier(String::new().into()), span)
    }

    #[cold]
    #[inline(never)]
    fn report_expected(&mut self, kind: &TokenKind) -> Span {
        let span = self.current_span();
        let message = format!("expected {kind}, found {}", self.peek());
        self.error(span, message);
        span
    }

    #[cold]
    #[inline(never)]
    fn report_expected_identifier(&mut self) {
        let span = self.current_span();
        let message = format!("expected identifier, found {}", self.peek());
        self.error(span, message);
    }

    #[cold]
    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(ParseError { span, message: message.into() });
    }

    /// Count one more level of nesting. Once nesting has reached
    /// [`MAX_NESTING_DEPTH`], record the resource-limit error instead and
    /// return false, leaving the count unchanged.
    #[must_use]
    fn enter_depth(&mut self) -> bool {
        // A host-stack budget, independent of target VM compiler limits.
        if self.nesting_depth >= MAX_NESTING_DEPTH {
            let span = self.current_span();
            self.error(
                span,
                format!(
                    "maximum nesting depth exceeded (luck parser resource limit: {MAX_NESTING_DEPTH})"
                ),
            );
            return false;
        }
        self.nesting_depth = self.nesting_depth.saturating_add(1);
        true
    }

    const fn exit_depth(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
    }

    /// Skip to the next statement start, block end, or closing bracket.
    /// Always consumes at least one token, so recovery can't loop forever.
    fn synchronize(&mut self) {
        self.advance();
        while !self.is_at_eof() {
            if stmt::is_statement_start(self.peek())
                || matches!(
                    self.peek(),
                    TokenKind::End
                        | TokenKind::Else
                        | TokenKind::ElseIf
                        | TokenKind::Until
                        | TokenKind::RightParen
                        | TokenKind::RightBrace
                        | TokenKind::RightBracket
                )
            {
                return;
            }
            self.advance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_source_storage_survives_repeated_parsing() {
        for text in ["return 1", "local = 5"] {
            let mut source = String::with_capacity(256);
            source.push_str(text);
            let pointer = source.as_ptr();
            let capacity = source.capacity();
            for _ in 0u8..3 {
                let parsed = parse(source, LuaVersion::Lua54);
                assert_eq!(parsed.source.as_ptr(), pointer);
                assert_eq!(parsed.source.capacity(), capacity);
                assert_eq!(parsed.source, text);
                assert_eq!(parsed.errors.is_empty(), text == "return 1", "{:?}", parsed.errors);
                source = parsed.source;
            }
        }
    }
}
