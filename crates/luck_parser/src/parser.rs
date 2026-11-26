mod attributes;
mod declaration;
mod definition;
mod depth;
mod expr;
mod forgotten;
mod function;
mod recovery;
mod stmt;
mod types;

use luck_allocator::{Allocator, Box, List, TextBytes, ThinBytes, ThinStr};
use luck_ast::{
    BinaryLink, Block, ElseIfClause, ElseIfExprClause, Expression, ExternTypeMember, Field,
    FunctionAttribute, FunctionTypeParam, GenericTypeParam, InterpolatedSegment, Name, NameIndex,
    NumberLiteral, Statement, StringLiteral, Suffix, TreeCounts, Type, TypeField, TypedName, Var,
};
use luck_diagnostics::Diagnostic;
use luck_lexer::Lexer;
use luck_token::{
    Span, StringValue, Token, TokenKind, decode_interpolated_text, decode_string, read_numeral,
};
use rustc_hash::FxHashMap;

use self::forgotten::{UnclosedBlock, label_forgotten_ends};
use self::recovery::OpenConstructs;
use crate::{Groupings, ParseResult};

/// The kinds [`TokenKind::is_name`] accepts, and any `other` kinds, as a
/// pattern, so a `match` that lists them keeps its jump table. It takes the
/// others itself because clippy rejects an or-pattern written around it.
macro_rules! name_token {
    ($($other:path)|*) => {
        TokenKind::Identifier
            | TokenKind::Continue
            | TokenKind::Export
            | TokenKind::Const
            | TokenKind::Type
            | TokenKind::Typeof
            | TokenKind::Read
            | TokenKind::Write
            $(| $other)*
    };
}
use name_token;

/// The tree and every list in it go in `allocator`, and its names and
/// literals borrow `source`, but for a string's value that decoding rewrote,
/// which goes in `allocator` too.
///
/// `source` is bytes, as Luau reads it, so a string or a comment in it may
/// hold any byte. It's code, where `declare` is an ordinary name, and
/// [`parse_definitions`] reads a definition file instead.
pub fn parse<'ast>(allocator: &'ast Allocator, source: &'ast [u8]) -> ParseResult<'ast> {
    parse_source(allocator, source, Tokens::Discard, SourceKind::Code)
}

/// [`parse`], keeping every token the parse read in
/// [`ParseResult::tokens`], for a host that needs both, such as the
/// formatter, so it lexes the source once.
pub fn parse_with_tokens<'ast>(
    allocator: &'ast Allocator,
    source: &'ast [u8],
) -> ParseResult<'ast> {
    parse_source(allocator, source, Tokens::Keep, SourceKind::Code)
}

/// [`parse`], for a definition file, as Luau's `Frontend::loadDefinitionFile`
/// reads one, with `allowDeclarationSyntax`: a statement may also be a
/// `declare`, and a function type in the type a `declare name: T` gives may
/// take attributes. Luau's parser takes either nowhere else.
pub fn parse_definitions<'ast>(
    allocator: &'ast Allocator,
    source: &'ast [u8],
) -> ParseResult<'ast> {
    parse_source(allocator, source, Tokens::Discard, SourceKind::Definitions)
}

/// [`parse_definitions`], keeping every token as [`parse_with_tokens`]
/// does.
pub fn parse_definitions_with_tokens<'ast>(
    allocator: &'ast Allocator,
    source: &'ast [u8],
) -> ParseResult<'ast> {
    parse_source(allocator, source, Tokens::Keep, SourceKind::Definitions)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tokens {
    Keep,
    Discard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Code,
    Definitions,
}

fn parse_source<'ast>(
    allocator: &'ast Allocator,
    source: &'ast [u8],
    tokens: Tokens,
    kind: SourceKind,
) -> ParseResult<'ast> {
    let lexer = match Lexer::new(source) {
        Ok(lexer) => lexer,
        Err(error) => {
            let block = Block::new(Span::new(0, 0), List::new());
            return ParseResult {
                source,
                chunk: block,
                comments: Vec::new(),
                tokens: Vec::new(),
                groupings: Groupings::default(),
                errors: vec![error],
                lenient_escapes: Vec::new(),
                // The empty block's own level.
                nesting_depth: 1,
                counts: TreeCounts::default(),
            };
        }
    };
    let tokens = (tokens == Tokens::Keep).then(|| Vec::with_capacity(lexer.token_capacity()));
    let placeholder = Token {
        span: Span::new(0, 0),
        kind: TokenKind::Eof,
        has_line_break_before: true,
        is_unterminated: false,
        has_escape: false,
    };
    let mut parser = Parser {
        allocator,
        source,
        text_source: TextBytes::new(source),
        lexer,
        is_definition_file: kind == SourceKind::Definitions,
        token: placeholder,
        previous_end: 0,
        nesting_depth: 0,
        deepest_level: 0,
        grouping_depth: 0,
        groupings: Groupings::default(),
        block_depth: 0,
        scratch: Scratch::default(),
        decoded: Vec::new(),
        function_scope: FunctionScope { is_chunk: true, is_vararg: true, is_in_loop: false },
        has_module_return: false,
        has_value_exports: false,
        exported_names: FxHashMap::default(),
        open_constructs: OpenConstructs::default(),
        has_syntax_error: false,
        errors: Vec::new(),
        unclosed_blocks: Vec::new(),
        tokens,
        counts: TreeCounts::default(),
    };
    parser.refill();
    parser.advance();
    parser.parse_chunk()
}

/// The grammar needs only one token of lookahead, so `token` and the
/// lexer's batch, which holds the token after it, stand in for a
/// materialized token stream, which measured slower to write and read back.
struct Parser<'ast> {
    allocator: &'ast Allocator,
    source: &'ast [u8],
    /// `source`, checked once, so a name in it needs no check of its own
    /// when it's all ASCII, and only of its ends when it's UTF-8.
    text_source: TextBytes<'ast>,
    /// Never all read between two tokens, so the lookahead is always in it.
    lexer: Lexer<'ast>,
    /// Whether a statement may be a `declare`, which the parser asks only of
    /// an identifier that opens one.
    is_definition_file: bool,
    token: Token,
    /// Where the last token read ends, where the node it ends does.
    previous_end: u32,
    /// Levels open where the parser is, as [`luck_ast::MAX_NESTING_DEPTH`]
    /// counts them.
    nesting_depth: usize,
    /// The deepest level opened since the operand being parsed began. A node
    /// that wraps a finished operand nests all of it one level deeper, so the
    /// wrapper counts from here.
    deepest_level: usize,
    /// Parentheses open where the parser is, which open no level, though the
    /// parser recurses through each.
    grouping_depth: usize,
    groupings: Groupings,
    /// The chunk's own block is 1.
    block_depth: u32,
    scratch: Scratch<'ast>,
    /// Where a string's value goes while decoding rewrites it, before the
    /// arena takes it, which leaves it empty for the next.
    decoded: Vec<u8>,
    function_scope: FunctionScope,
    has_module_return: bool,
    has_value_exports: bool,
    /// Where each exported name is declared.
    exported_names: FxHashMap<&'ast str, Span>,
    open_constructs: OpenConstructs,
    /// Whether a syntax error has been reported, or a string cut off before
    /// its closer read, since the last statement began where one reliably
    /// does.
    has_syntax_error: bool,
    errors: Vec<Diagnostic>,
    unclosed_blocks: Vec<UnclosedBlock>,
    /// Every token lexed so far, for `parse_with_tokens`.
    tokens: Option<Vec<Token>>,
    counts: TreeCounts,
}

/// The items of the lists being parsed, innermost list last.
///
/// Each list takes its own off the end when it closes, so it gets one exact
/// allocation in the arena however long it grows, and nesting costs no
/// stack. The commonest lists, of one item and of some kinds two, skip the
/// copy out of here and move to the arena as values.
#[derive(Default)]
struct Scratch<'ast> {
    stmts: Vec<Statement<'ast>>,
    exprs: Vec<Expression<'ast>>,
    binary_links: Vec<BinaryLink<'ast>>,
    suffixes: Vec<Suffix<'ast>>,
    fields: Vec<Field<'ast>>,
    vars: Vec<Var<'ast>>,
    names: Vec<Name<'ast>>,
    typed_names: Vec<TypedName<'ast>>,
    attributes: Vec<FunctionAttribute<'ast>>,
    elseif_clauses: Vec<ElseIfClause<'ast>>,
    elseif_exprs: Vec<ElseIfExprClause<'ast>>,
    segments: Vec<InterpolatedSegment<'ast>>,
    types: Vec<Type<'ast>>,
    type_fields: Vec<TypeField<'ast>>,
    extern_type_members: Vec<ExternTypeMember<'ast>>,
    function_type_params: Vec<FunctionTypeParam<'ast>>,
    generic_params: Vec<GenericTypeParam<'ast>>,
}

/// What the innermost function body, or the main chunk, allows. Entering a
/// function body swaps in a fresh one.
#[derive(Clone, Copy)]
struct FunctionScope {
    /// Whether this is the main chunk, whose `return` is the module's.
    is_chunk: bool,
    /// Whether `...` is valid. The main chunk is vararg; a function body is
    /// only when its params end in `...`.
    is_vararg: bool,
    /// Whether a loop body encloses this point. `break` or `continue` outside
    /// one is a compile error.
    is_in_loop: bool,
}

impl<'ast> Parser<'ast> {
    /// Nothing is open around the chunk's block, so it reads to the end of
    /// the source, and the lexer is drained by then.
    fn parse_chunk(mut self) -> ParseResult<'ast> {
        let start_span = self.token.span;
        let chunk = self.parse_block().unwrap_or_else(|| Block::new(start_span, List::new()));
        if !self.unclosed_blocks.is_empty() {
            label_forgotten_ends(&chunk, self.source, &self.unclosed_blocks, &mut self.errors);
        }
        let mut tokens = self.tokens.unwrap_or_default();
        // The lexer repeats its `Eof` to a parser that reads on.
        while let [.., Token { kind: TokenKind::Eof, .. }, Token { kind: TokenKind::Eof, .. }] =
            tokens.as_slice()
        {
            tokens.pop();
        }
        let lexed = self.lexer.finish(tokens);
        let mut errors = lexed.errors;
        errors.append(&mut self.errors);
        errors.sort_by_key(|e| e.span.map(|span| span.start));
        self.groupings.finish();
        ParseResult {
            source: self.source,
            chunk,
            comments: lexed.comments,
            tokens: lexed.tokens,
            groupings: self.groupings,
            errors,
            lenient_escapes: lexed.lenient_escapes,
            nesting_depth: self.deepest_level,
            counts: self.counts,
        }
    }

    #[inline]
    const fn peek(&self) -> TokenKind {
        self.token.kind
    }

    #[inline]
    fn peek_next(&self) -> TokenKind {
        self.lexer.peek_buffered().kind
    }

    #[inline]
    fn is_at(&self, kind: TokenKind) -> bool {
        self.token.kind == kind
    }

    #[inline]
    fn is_at_eof(&self) -> bool {
        self.is_at(TokenKind::Eof)
    }

    /// Whether the current token can be a name: an identifier, or a
    /// contextual keyword.
    #[inline]
    const fn is_at_name(&self) -> bool {
        self.token.kind.is_name()
    }

    /// Whether the current token is spelled `word`, which no token of
    /// another kind is when `word` is an identifier's spelling.
    #[inline]
    fn is_at_word(&self, word: &[u8]) -> bool {
        self.token.span.text(self.source) == word
    }

    /// The text of the name or numeral at `span`, which the lexer read as
    /// ASCII.
    #[inline]
    fn ascii_text(&self, span: Span) -> ThinStr<'ast> {
        self.text_source.text(span.into()).expect("the lexer reads names and numerals as ASCII")
    }

    /// Consume the current token and return its span. The token after it
    /// becomes the current one, copied out of the lexer's batch, which is
    /// refilled as soon as it's all read. While the batch lasts it calls
    /// nothing, so it saves no register, and it reaches the next batch
    /// through a tail call.
    #[inline(never)]
    fn advance(&mut self) -> Span {
        let span = self.token.span;
        self.previous_end = span.end;
        self.token = self.lexer.take_buffered();
        if self.lexer.has_buffered() { span } else { self.refill_after(span) }
    }

    /// [`Self::advance`] once it has taken the last token of the batch,
    /// which returns its `span`.
    #[cold]
    #[inline(never)]
    fn refill_after(&mut self, span: Span) -> Span {
        self.refill();
        span
    }

    /// Lex the next batch, which `tokens` keeps for `parse_with_tokens`.
    fn refill(&mut self) {
        let batch = self.lexer.refill();
        if let Some(tokens) = &mut self.tokens {
            tokens.extend_from_slice(batch);
        }
    }

    /// Consume the current token if it is a `kind`.
    #[inline]
    fn eat(&mut self, kind: TokenKind) -> bool {
        let is_at_kind = self.is_at(kind);
        if is_at_kind {
            self.advance();
        }
        is_at_kind
    }

    /// Consume a token of `kind` and return its span. On a mismatch, record
    /// an "expected X, found Y" error, leave the token in place, and return
    /// its span so parsing can go on.
    #[inline]
    fn expect(&mut self, kind: TokenKind) -> Span {
        if self.is_at(kind) { self.advance() } else { self.report_expected(kind) }
    }

    /// Consume the current token if it can be a name. Otherwise record an
    /// "expected identifier" error and leave the token in place.
    #[inline]
    fn expect_identifier(&mut self) -> Option<Name<'ast>> {
        if self.is_at_name() {
            Some(self.advance_name())
        } else {
            self.report_expected(TokenKind::Identifier);
            None
        }
    }

    /// Like [`Self::expect_identifier`], but a missing name is an empty one
    /// at the current token.
    #[inline]
    fn expect_identifier_recover(&mut self) -> Name<'ast> {
        if self.is_at_name() {
            self.advance_name()
        } else {
            self.report_expected(TokenKind::Identifier);
            Name::new(ThinStr::new(""), None, self.token.span)
        }
    }

    /// Consume the current number, whose kind the caller has matched, as a
    /// literal holding its value.
    #[inline]
    fn advance_number(&mut self) -> Box<'ast, NumberLiteral<'ast>> {
        let span = self.advance();
        let raw = self.ascii_text(span);
        let numeral = read_numeral(raw.as_str());
        let literal = NumberLiteral::new(numeral.value, Some(raw), span, numeral.is_exact);
        self.alloc(literal)
    }

    /// Consume the current string, whose kind the caller has matched, as a
    /// literal holding its value.
    ///
    /// Every string the parser reads as one comes through here or
    /// [`Self::interpolated_text`], so they mark a string cut off before its
    /// closer, rather than [`Self::advance`] checking every token.
    #[inline]
    fn advance_string(&mut self) -> Box<'ast, StringLiteral<'ast>> {
        if self.token.is_unterminated {
            self.has_syntax_error = true;
        }
        let has_escape = self.token.has_escape;
        let span = self.advance();
        let raw = span.text(self.source);
        let value = match decode_string(raw, has_escape, &mut self.decoded) {
            StringValue::Verbatim(bytes) => bytes,
            StringValue::Decoded => self.take_decoded(),
        };
        self.alloc(StringLiteral::new(ThinBytes::new(value), Some(ThinBytes::new(raw)), span))
    }

    /// An interpolated string's piece of text, between its delimiters, as a
    /// literal holding its value.
    fn interpolated_text(&mut self, token: Token) -> StringLiteral<'ast> {
        if token.is_unterminated {
            self.has_syntax_error = true;
        }
        let span = token.text_span();
        let raw = span.text(self.source);
        let value = match decode_interpolated_text(raw, token.has_escape, &mut self.decoded) {
            StringValue::Verbatim(bytes) => bytes,
            StringValue::Decoded => self.take_decoded(),
        };
        StringLiteral::new(ThinBytes::new(value), Some(ThinBytes::new(raw)), span)
    }

    /// The value decoding wrote, moved to the arena.
    fn take_decoded(&mut self) -> &'ast [u8] {
        let value = self.allocator.alloc_slice_copy(&self.decoded);
        self.decoded.clear();
        value
    }

    /// Consume the current identifier or contextual keyword as a name. The
    /// caller has already matched its kind.
    #[inline]
    fn advance_name(&mut self) -> Name<'ast> {
        let span = self.advance();
        let index = self.counts.names;
        self.counts.names = index + 1;
        Name::new(self.ascii_text(span), Some(NameIndex::new(index)), span)
    }

    #[inline]
    fn alloc<T>(&self, value: T) -> Box<'ast, T> {
        Box::new_in(value, self.allocator)
    }
}
