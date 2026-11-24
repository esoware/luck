mod attributes;
mod declaration;
mod expr;
mod forgotten;
mod function;
mod stmt;
mod types;

use std::borrow::Cow;
use std::mem;

use luck_allocator::{Allocator, Box, List, ThinBytes, ThinStr};
use luck_ast::{
    AttributedName, BinaryLink, Block, Chunk, ElseIfClause, ElseIfExprClause, Expression, Field,
    FunctionAttribute, FunctionTypeParam, GenericTypeParam, IntegerLiteral, InterpolatedSegment,
    MAX_NESTING_DEPTH, Name, NameIndex, NumberLiteral, Parameter, Statement, StringLiteral, Suffix,
    TreeCounts, Type, TypeField, Var,
};
use luck_diagnostics::Diagnostic;
use luck_lexer::Lexer;
use luck_token::{
    LuaVersion, Span, StringValue, Token, TokenKind, decode_interpolated_text, decode_string,
    read_integer, read_numeral,
};
use rustc_hash::FxHashMap;

use self::forgotten::label_forgotten_ends;
use crate::{Groupings, ParseResult};

/// The tree and every list in it go in `allocator`, and its names and
/// literals borrow `source`, but for a string's value that decoding rewrote,
/// which goes in `allocator` too.
///
/// `source` is bytes, as Lua reads it, so a string or a comment in it may
/// hold any byte.
pub fn parse<'ast>(
    allocator: &'ast Allocator,
    source: &'ast [u8],
    version: LuaVersion,
) -> ParseResult<'ast> {
    parse_recording(allocator, source, version, false)
}

/// [`parse`], keeping every token the parse read in
/// [`ParseResult::tokens`], for a host that needs both, such as the
/// formatter, so it lexes the source once.
pub fn parse_with_tokens<'ast>(
    allocator: &'ast Allocator,
    source: &'ast [u8],
    version: LuaVersion,
) -> ParseResult<'ast> {
    parse_recording(allocator, source, version, true)
}

fn parse_recording<'ast>(
    allocator: &'ast Allocator,
    source: &'ast [u8],
    version: LuaVersion,
    is_recording: bool,
) -> ParseResult<'ast> {
    let lexer = match Lexer::new(source, version) {
        Ok(lexer) => lexer,
        Err(error) => {
            let block = Block::new(Span::new(0, 0), List::new());
            return ParseResult {
                source,
                chunk: Chunk::new(version, block),
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
    let tokens = is_recording.then(|| Vec::with_capacity(lexer.token_capacity()));
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
        lexer,
        token: placeholder,
        next: placeholder,
        previous_span: Span::new(0, 0),
        version,
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
        open: OpenConstructs::default(),
        has_syntax_error: false,
        errors: Vec::new(),
        unclosed_blocks: Vec::new(),
        tokens,
        counts: TreeCounts::default(),
    };
    // `advance` is the lexer's one caller, so it's inlined there alone.
    parser.advance();
    parser.advance();
    parser.previous_span = Span::new(0, 0);
    parser.parse_chunk()
}

/// Lua needs only one token of lookahead, so `token` and `next` stand in for
/// a materialized token stream, which measured slower to write and read back.
struct Parser<'ast> {
    allocator: &'ast Allocator,
    source: &'ast [u8],
    lexer: Lexer<'ast>,
    token: Token,
    next: Token,
    previous_span: Span,
    version: LuaVersion,
    /// Levels open where the parser is, as [`MAX_NESTING_DEPTH`] counts them.
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
    open: OpenConstructs,
    /// Whether a syntax error has been reported, or a string cut off before
    /// its closer read, since the last statement began where one reliably
    /// does.
    has_syntax_error: bool,
    errors: Vec<Diagnostic>,
    /// Each error that a block is missing its `end` or `until`, as its index
    /// in `errors` and the span of the token that opened the block.
    unclosed_blocks: Vec<(usize, Span)>,
    /// Every token lexed so far, for `parse_with_tokens`.
    tokens: Option<Vec<Token>>,
    counts: TreeCounts,
}

/// The items of every list being parsed, innermost list last.
///
/// Each list takes its own off the end when it closes, so it gets one exact
/// allocation in the arena however long it grows, and nesting costs no
/// stack.
#[derive(Default)]
struct Scratch<'ast> {
    stmts: Vec<Statement<'ast>>,
    exprs: Vec<Expression<'ast>>,
    binary_links: Vec<BinaryLink<'ast>>,
    suffixes: Vec<Suffix<'ast>>,
    fields: Vec<Field<'ast>>,
    vars: Vec<Var<'ast>>,
    names: Vec<Name<'ast>>,
    params: Vec<Parameter<'ast>>,
    attributed_names: Vec<AttributedName<'ast>>,
    attributes: Vec<FunctionAttribute<'ast>>,
    elseif_clauses: Vec<ElseIfClause<'ast>>,
    elseif_exprs: Vec<ElseIfExprClause<'ast>>,
    segments: Vec<InterpolatedSegment<'ast>>,
    types: Vec<Type<'ast>>,
    type_fields: Vec<TypeField<'ast>>,
    function_type_params: Vec<FunctionTypeParam<'ast>>,
    generic_params: Vec<GenericTypeParam<'ast>>,
}

/// A token that closes a construct, or continues it, as `else` does an `if`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Closer {
    Paren,
    Bracket,
    Brace,
    /// An interpolated string's next segment, after an expression.
    StringSegment,
    /// `end` or `until`, each of which ends any block, as in every Lua's
    /// parser, so a mistaken one ends the block it's in, and the construct
    /// around that block reports what it was missing.
    BlockEnd,
    /// `else` or `elseif`.
    Else,
}

impl Closer {
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token closes nothing")]
    const fn of(kind: TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::RightParen => Self::Paren,
            TokenKind::RightBracket => Self::Bracket,
            TokenKind::RightBrace => Self::Brace,
            TokenKind::InterpolatedStringMiddle | TokenKind::InterpolatedStringEnd => {
                Self::StringSegment
            }
            TokenKind::End | TokenKind::Until => Self::BlockEnd,
            TokenKind::Else | TokenKind::ElseIf => Self::Else,
            _ => return None,
        })
    }
}

/// How many constructs the parser is inside that each closer closes, so
/// recovery can tell a closer something open expects from a stray one.
#[derive(Clone, Copy, Default)]
struct OpenConstructs {
    parens: u32,
    brackets: u32,
    braces: u32,
    string_segments: u32,
    block_ends: u32,
    elses: u32,
}

impl OpenConstructs {
    const fn open(&mut self, closer: Closer) {
        *self.count_mut(closer) += 1;
    }

    const fn close(&mut self, closer: Closer) {
        *self.count_mut(closer) -= 1;
    }

    const fn count_mut(&mut self, closer: Closer) -> &mut u32 {
        match closer {
            Closer::Paren => &mut self.parens,
            Closer::Bracket => &mut self.brackets,
            Closer::Brace => &mut self.braces,
            Closer::StringSegment => &mut self.string_segments,
            Closer::BlockEnd => &mut self.block_ends,
            Closer::Else => &mut self.elses,
        }
    }

    const fn expects(self, closer: Closer) -> bool {
        let count = match closer {
            Closer::Paren => self.parens,
            Closer::Bracket => self.brackets,
            Closer::Brace => self.braces,
            Closer::StringSegment => self.string_segments,
            Closer::BlockEnd => self.block_ends,
            Closer::Else => self.elses,
        };
        count > 0
    }
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
    /// one is a compile error in every Lua.
    is_in_loop: bool,
}

impl<'ast> Parser<'ast> {
    /// Nothing is open around the chunk's block, so it reads to the end of
    /// the source, and the lexer is drained by then.
    fn parse_chunk(mut self) -> ParseResult<'ast> {
        let start_span = self.token.span;
        let block = self.parse_block().unwrap_or_else(|| Block::new(start_span, List::new()));
        let chunk = Chunk::new(self.version, block);
        if !self.unclosed_blocks.is_empty() {
            label_forgotten_ends(&chunk, self.source, &self.unclosed_blocks, &mut self.errors);
        }
        let lexed = self.lexer.finish();
        let mut errors = lexed.errors;
        errors.append(&mut self.errors);
        errors.sort_by_key(|e| e.span.map(|span| span.start));
        self.groupings.finish();
        let mut tokens = self.tokens.unwrap_or_default();
        // The lexer repeats its `Eof` to a parser that reads on.
        while let [.., Token { kind: TokenKind::Eof, .. }, Token { kind: TokenKind::Eof, .. }] =
            tokens.as_slice()
        {
            tokens.pop();
        }
        ParseResult {
            source: self.source,
            chunk,
            comments: lexed.comments,
            tokens,
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
    const fn peek_next(&self) -> TokenKind {
        self.next.kind
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

    /// The text of the name or numeral at `span`, which the lexer read as
    /// ASCII.
    #[inline]
    fn ascii_text(&self, span: Span) -> ThinStr<'ast> {
        ThinStr::from_ascii(span.text(self.source))
            .expect("the lexer reads names and numerals as ASCII")
    }

    /// Consume the current token and return its span. Out of line, so the
    /// lexer it inlines is inlined once: inlined into every caller, it parses
    /// 5% slower.
    #[inline(never)]
    fn advance(&mut self) -> Span {
        let span = self.token.span;
        self.previous_span = span;
        self.token = self.next;
        self.next = self.lexer.next_token();
        if let Some(tokens) = &mut self.tokens {
            tokens.push(self.next);
        }
        span
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

    /// Consume the current number, whose kind the caller has matched, as a
    /// literal holding its value.
    #[inline]
    fn advance_number(&mut self) -> Box<'ast, NumberLiteral<'ast>> {
        let span = self.advance();
        let raw = self.ascii_text(span);
        let numeral = read_numeral(raw.as_str(), self.version);
        let literal = NumberLiteral::new(numeral.value, Some(raw), span, numeral.is_exact);
        self.alloc(literal)
    }

    /// Luau: consume the current integer, whose kind the caller has matched,
    /// which the lexer checked fits 64 bits.
    fn advance_integer(&mut self) -> Box<'ast, IntegerLiteral<'ast>> {
        let span = self.advance();
        let raw = self.ascii_text(span);
        let value = read_integer(raw.as_str()).expect("the lexer checked the integer fits 64 bits");
        self.alloc(IntegerLiteral::new(value, Some(raw), span))
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
        let value = match decode_string(raw, has_escape, self.version, &mut self.decoded) {
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
        let decoding =
            decode_interpolated_text(raw, token.has_escape, self.version, &mut self.decoded);
        let value = match decoding {
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

    /// Whether a syntax error was reported, or a string or block comment cut
    /// off before its closer passed, since the last statement began where
    /// one reliably does. Errors that follow either in the statement would
    /// only echo it: a cut-off string took the rest of its line, and a
    /// cut-off comment the rest of the source, often with the closer of what
    /// holds it, so only `Eof` follows that.
    #[inline]
    fn is_recovering(&self) -> bool {
        self.has_syntax_error || (self.token.is_unterminated && self.is_at_eof())
    }

    /// Consume a token of `kind` and return its span. On a mismatch, record
    /// an "expected X, found Y" error, leave the token in place, and return
    /// its span so parsing can go on.
    #[inline]
    fn expect(&mut self, kind: TokenKind) -> Span {
        if self.is_at(kind) { self.advance() } else { self.report_expected(kind) }
    }

    /// Count a construct `closer` closes as open from here.
    const fn open(&mut self, closer: Closer) {
        self.open.open(closer);
    }

    /// Count the construct `closer` closes as closed from here.
    const fn close(&mut self, closer: Closer) {
        self.open.close(closer);
    }

    /// Close the construct `closer` closes and consume its `kind` closer, as
    /// [`Self::expect`] does, but one that's missing is an error that points
    /// at `opener` too, the token that opened the construct.
    #[inline]
    fn expect_closer(&mut self, kind: TokenKind, closer: Closer, opener: Span) -> Span {
        self.close(closer);
        if self.is_at(kind) { self.advance() } else { self.report_missing_closer(kind, opener) }
    }

    /// A closing bracket of another kind that nothing open expects is a typo
    /// for a missing bracket, so it closes the bracket too.
    #[cold]
    #[inline(never)]
    fn report_missing_closer(&mut self, kind: TokenKind, opener: Span) -> Span {
        let span = self.report_unclosed(kind, opener);
        let is_bracket = |closer| {
            matches!(
                closer,
                TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
            )
        };
        if is_bracket(kind) && is_bracket(self.peek()) && !self.is_at_expected_closer() {
            self.advance();
        }
        span
    }

    /// Record that the current token isn't the `kind` that closes what
    /// `opener` opened, as PUC Lua and Luau report it, with the opener
    /// labeled. Leaves the token in place and returns its span.
    #[cold]
    #[inline(never)]
    fn report_unclosed(&mut self, kind: TokenKind, opener: Span) -> Span {
        let span = self.token.span;
        if !self.is_recovering() {
            self.has_syntax_error = true;
            let opener_text = String::from_utf8_lossy(opener.text(self.source));
            let message = format!(
                "expected {kind} to close `{opener_text}`, found {}",
                self.token.display(self.source)
            );
            let diag = Diagnostic::e008(span, message)
                .with_label(opener, format!("unclosed `{opener_text}`"));
            if matches!(kind, TokenKind::End | TokenKind::Until) {
                self.unclosed_blocks.push((self.errors.len(), opener));
            }
            self.errors.push(diag);
        }
        span
    }

    /// Whether the current token closes or continues something open.
    fn is_at_expected_closer(&self) -> bool {
        Closer::of(self.peek()).is_some_and(|closer| self.open.expects(closer))
    }

    /// Whether the block being parsed ends at the current token.
    fn is_at_block_end(&self) -> bool {
        self.is_at_eof() || self.is_at_expected_closer()
    }

    /// Consume the current token if it can be a name. Otherwise record an
    /// "expected identifier" error and leave the token in place.
    #[inline]
    fn expect_identifier(&mut self) -> Option<Name<'ast>> {
        if self.is_at_name() {
            Some(self.advance_name())
        } else {
            self.report_expected_identifier();
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
            self.report_expected_identifier();
            Name::new(ThinStr::new(""), None, self.token.span)
        }
    }

    /// The message is only built when it's reported.
    #[cold]
    #[inline(never)]
    fn report_expected(&mut self, kind: TokenKind) -> Span {
        let span = self.token.span;
        if !self.is_recovering() {
            self.has_syntax_error = true;
            let message = format!("expected {kind}, found {}", self.token.display(self.source));
            self.error(span, message);
        }
        span
    }

    #[cold]
    #[inline(never)]
    fn report_expected_identifier(&mut self) {
        if !self.is_recovering() {
            self.has_syntax_error = true;
            let span = self.token.span;
            let message = format!("expected identifier, found {}", self.token.display(self.source));
            self.error(span, message);
        }
    }

    #[cold]
    #[inline(never)]
    fn error<Message>(&mut self, span: Span, message: Message)
    where
        Message: Into<Cow<'static, str>>,
    {
        self.errors.push(Diagnostic::e008(span, message));
    }

    /// Record a syntax error unless one is already being recovered from.
    #[cold]
    #[inline(never)]
    fn syntax_error<Message>(&mut self, span: Span, message: Message)
    where
        Message: Into<Cow<'static, str>>,
    {
        if !self.is_recovering() {
            self.has_syntax_error = true;
            self.error(span, message);
        }
    }

    /// Count one more level of nesting, or at [`MAX_NESTING_DEPTH`], stop
    /// the parse there and return false, leaving the count unchanged.
    #[inline]
    fn enter_depth(&mut self) -> bool {
        // A host-stack budget, independent of target VM compiler limits.
        if self.nesting_depth >= MAX_NESTING_DEPTH {
            self.stop_at_depth_limit();
            return false;
        }
        self.nesting_depth += 1;
        self.deepest_level = self.deepest_level.max(self.nesting_depth);
        true
    }

    /// Count one more pair of parentheses open, or at [`MAX_NESTING_DEPTH`]
    /// of them, stop the parse as [`Self::enter_depth`] does.
    ///
    /// The tree leaves them out, so they have a cap of their own: a tree
    /// within its cap prints with at most a pair at each of its levels, and
    /// reads back.
    #[inline]
    fn enter_grouping(&mut self) -> bool {
        if self.grouping_depth >= MAX_NESTING_DEPTH {
            self.stop_at_depth_limit();
            return false;
        }
        self.grouping_depth += 1;
        true
    }

    #[inline]
    const fn exit_depth(&mut self) {
        self.nesting_depth -= 1;
    }

    /// The cap is fatal: one error at the current token, and the rest of the
    /// source is skipped, so every construct still open ends at the end of
    /// the source, where recovery reports nothing more.
    #[cold]
    #[inline(never)]
    fn stop_at_depth_limit(&mut self) {
        let span = self.token.span;
        self.syntax_error(
            span,
            format!(
                "maximum nesting depth exceeded (luck parser resource limit: {MAX_NESTING_DEPTH})"
            ),
        );
        while !self.is_at_eof() {
            self.advance();
        }
    }

    /// Start measuring an operand at the current level, returning the
    /// deepest level of the operand around it. Hand that to
    /// [`Self::end_operand`] once the operand is whole.
    #[inline]
    const fn begin_operand(&mut self) -> usize {
        mem::replace(&mut self.deepest_level, self.nesting_depth)
    }

    /// Fold the finished operand's depth into that of the operand around it.
    #[inline]
    fn end_operand(&mut self, outer_deepest_level: usize) {
        self.deepest_level = self.deepest_level.max(outer_deepest_level);
    }

    /// Count a node that wraps the operand parsed so far at `level`: a run of
    /// binary operators or suffixes, a cast, parentheses the tree keeps, or a
    /// type operator. Everything in the operand moves one level down, so the
    /// cap applies to its deepest level, and stops the parse as
    /// [`Self::enter_depth`] does.
    ///
    /// On success the parser stands one level in, where the wrapper's other
    /// children go, and the caller steps back to `level` after them.
    #[inline]
    fn enter_wrapper(&mut self, level: usize) -> bool {
        if self.deepest_level >= MAX_NESTING_DEPTH {
            self.stop_at_depth_limit();
            return false;
        }
        self.deepest_level += 1;
        self.nesting_depth = level + 1;
        true
    }

    /// Skip to the block's end, or to where a statement reliably begins and
    /// recovery ends. False when it skipped, so the block looks again.
    #[cold]
    #[inline(never)]
    fn recover_at_statement(&mut self) -> bool {
        if self.is_at_block_end() {
            return true;
        }
        if !self.is_at_reliable_stmt_start() {
            self.skip_to_statement();
            return false;
        }
        self.has_syntax_error = false;
        true
    }

    /// Skip to where a statement reliably begins, or to a closer something
    /// open expects. A name or a `(` on the line of an error may be what's
    /// left of the statement it broke, so one begins a statement only at the
    /// start of a line.
    fn skip_to_statement(&mut self) {
        while !self.is_at_block_end() && !self.is_at_reliable_stmt_start() {
            self.advance();
        }
    }

    const fn is_at_reliable_stmt_start(&self) -> bool {
        let kind = self.peek();
        stmt::is_stmt_start(kind)
            && (!(kind.is_name() || matches!(kind, TokenKind::LeftParen))
                || self.token.has_line_break_before)
    }
}

#[cfg(test)]
mod tests {
    use luck_ast::block_nesting_depth;
    use luck_testgen::{generate_full_grammar, generate_runtime_safe, lua_fixtures};

    use super::*;

    /// The depth the parser counted and the depth of the tree it built.
    fn counted_and_built(source: &str, version: LuaVersion) -> (usize, usize) {
        let allocator = Allocator::new();
        let result = parse(&allocator, source.as_bytes(), version);
        assert!(result.errors.is_empty(), "{:?}\n{source}", result.errors.first());
        (result.nesting_depth, block_nesting_depth(&result.chunk.block))
    }

    #[test]
    fn counts_the_depth_of_each_construct_as_the_tree_measures_it() {
        for (version, source) in [
            (LuaVersion::Lua54, "return ((a + b) * -c)^2 .. d"),
            (LuaVersion::Lua54, "return a.b[c](d):e{f}\"g\""),
            (LuaVersion::Lua54, "return f{g{h}}, f({g({h})})"),
            (LuaVersion::Lua54, "return {a = {b = {[c] = {d}}}}"),
            (LuaVersion::Lua54, "local function f(...) return function(...) return ... end end"),
            (LuaVersion::Lua54, "a.b.c[d.e].f = g; h(i).j = k"),
            (LuaVersion::Lua54, "for i = a.b, c(d), -e do while f do repeat until g end end"),
            (LuaVersion::Lua54, "if a then elseif b.c then if d then end else do end end"),
            (LuaVersion::Luau, "return `a{b}c{`d{e + f}`}g`, `plain`"),
            (LuaVersion::Luau, "return if a then b else if c then d else e"),
            (LuaVersion::Luau, "return (a :: T) :: U, b :: {V}, f<<T, {U}>>(c), d:m<<T>>()"),
            (LuaVersion::Luau, "type A = {B}? | (C & D) | typeof(e.f) | M.N<O, P<Q>>"),
            (LuaVersion::Luau, "type A = | B | {C}?? type D = & E & (F) -> (G, ...H)"),
            (LuaVersion::Luau, "type A<T = {U}, V... = (W, X)> = <Y>(z: Y, ...{Y}) -> ...Y"),
            (LuaVersion::Luau, "local function f<T>(a: {T}, ...: T?): (T?, ...{T}) end"),
            (LuaVersion::Luau, "local function g(): ({T} | V, ...U) end"),
            (LuaVersion::Luau, "@native @[deprecated{use = 'a'}] local function f() end"),
            (LuaVersion::Luau, "return @[deprecated({reason = 'b'})] function() end"),
            (LuaVersion::Luau, "type function f(a: {b}) return a end"),
            (LuaVersion::Luau, "for i: {T}, v: T? in a do x += f(y) end"),
        ] {
            let (counted, built) = counted_and_built(source, version);
            assert_eq!(counted, built, "{source}");
        }
    }

    #[test]
    fn a_suffix_folded_into_a_return_pack_counts_from_the_whole_pack() {
        // `(T, ...{U})?` parses as `(T?, ...{U})`, and by then only the
        // pack's depth is known, not its first element's.
        let folded = "local function f(): (T, ...{U})? end";
        let written = "local function f(): (T?, ...{U}) end";
        let (counted, built) = counted_and_built(folded, LuaVersion::Luau);
        assert_eq!((counted, built), (6, 5));
        assert_eq!(counted_and_built(written, LuaVersion::Luau), (5, 5));
    }

    #[test]
    fn counts_the_depth_of_generated_programs_as_the_tree_measures_it() {
        for version in LuaVersion::ALL {
            for seed in 0..40 {
                for source in [
                    generate_full_grammar(seed, version, 25),
                    generate_runtime_safe(seed, version, 25),
                ] {
                    let (counted, built) = counted_and_built(&source, version);
                    assert_eq!(counted, built, "seed {seed} {version:?}\n{source}");
                }
            }
        }
    }

    #[test]
    fn counts_the_depth_of_fixtures_as_the_tree_measures_it() {
        let mut checked = 0;
        for fixture in lua_fixtures() {
            if fixture.is_error {
                continue;
            }
            let (counted, built) = counted_and_built(&fixture.source, fixture.version);
            assert_eq!(counted, built, "{}", fixture.path.display());
            checked += 1;
        }
        assert!(checked > 0, "no fixture outside `errors/`");
    }
}
