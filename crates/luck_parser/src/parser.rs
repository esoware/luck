mod attributes;
mod declaration;
mod expr;
mod function;
mod stmt;
mod types;

use core::mem;

use luck_allocator::{Allocator, Box, List, ThinStr};
use luck_ast::{
    AttributedName, BinaryLink, Block, Chunk, ElseIfClause, ElseIfExprClause, Expression, Field,
    FunctionAttribute, FunctionTypeParam, GenericTypeParam, InterpolatedSegment, Literal,
    MAX_NESTING_DEPTH, Name, NameIndex, Parameter, Statement, Suffix, TreeCounts, Type, TypeField,
    Var,
};
use luck_lexer::Lexer;
use luck_token::{LuaVersion, Span, Token, TokenKind};
use rustc_hash::FxHashSet;

use crate::validate::check_type_functions;
use crate::{ParseError, ParseResult};

/// The tree and every list in it go in `allocator`, and its names and
/// literals borrow `source`.
#[must_use]
pub fn parse<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
) -> ParseResult<'ast> {
    parse_recording(allocator, source, version, false).0
}

/// [`parse`], and every token the parse read, as [`luck_lexer::lex`] gives
/// them, for a host that needs both, so it lexes the source once.
#[must_use]
pub fn parse_with_tokens<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
) -> (ParseResult<'ast>, Vec<Token>) {
    let (parse, recorded) = parse_recording(allocator, source, version, true);
    let mut tokens = recorded.unwrap_or_default();
    // The lexer repeats its `Eof` to a parser that reads on.
    while let [.., Token { kind: TokenKind::Eof, .. }, Token { kind: TokenKind::Eof, .. }] =
        tokens.as_slice()
    {
        tokens.pop();
    }
    (parse, tokens)
}

fn parse_recording<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
    is_recording: bool,
) -> (ParseResult<'ast>, Option<Vec<Token>>) {
    let lexer = match Lexer::new(source, version) {
        Ok(lexer) => lexer,
        Err(error) => {
            let block = Block::new(Span::new(0, 0), List::new());
            let parse = ParseResult {
                source,
                chunk: Chunk::new(version, block),
                comments: Vec::new(),
                errors: vec![error],
                // The empty block's own level.
                nesting_depth: 1,
                counts: TreeCounts::default(),
            };
            return (parse, is_recording.then(Vec::new));
        }
    };
    let tokens = is_recording.then(|| Vec::with_capacity(lexer.token_capacity()));
    let placeholder = Token {
        span: Span::new(0, 0),
        kind: TokenKind::Eof,
        has_line_break_before: true,
        is_unterminated: false,
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
        block_depth: 0,
        scratch: Scratch::default(),
        function_scope: FunctionScope { is_chunk: true, is_vararg: true, is_in_loop: false },
        has_module_return: false,
        has_value_exports: false,
        exported_names: FxHashSet::default(),
        has_type_function: false,
        open: OpenConstructs::default(),
        has_syntax_error: false,
        settled_unterminated: 0,
        errors: Vec::new(),
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
#[expect(clippy::struct_excessive_bools, reason = "each bool is a separate fact about the parse")]
struct Parser<'ast> {
    allocator: &'ast Allocator,
    source: &'ast str,
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
    /// The chunk's own block is 1.
    block_depth: u32,
    scratch: Scratch<'ast>,
    function_scope: FunctionScope,
    has_module_return: bool,
    has_value_exports: bool,
    exported_names: FxHashSet<&'ast str>,
    /// Whether a `type function` appeared, so its body needs the scope check
    /// the parser can't do without tracking locals.
    has_type_function: bool,
    open: OpenConstructs,
    /// Whether a syntax error has been reported since the last statement
    /// began where one reliably does.
    has_syntax_error: bool,
    /// How many of the lexer's unterminated strings and block comments come
    /// before that statement.
    settled_unterminated: usize,
    errors: Vec<ParseError>,
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
    /// Each open pairs with a later close, and no more constructs nest than
    /// the source has tokens, so a count never wraps.
    const fn open(&mut self, closer: Closer) {
        let count = self.count_mut(closer);
        *count = count.wrapping_add(1);
    }

    const fn close(&mut self, closer: Closer) {
        let count = self.count_mut(closer);
        *count = count.wrapping_sub(1);
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
    fn parse_chunk(mut self) -> (ParseResult<'ast>, Option<Vec<Token>>) {
        let start_span = self.token.span;
        let block = self.parse_block().unwrap_or_else(|| Block::new(start_span, List::new()));
        let chunk = Chunk::new(self.version, block);
        let (comments, mut errors) = self.lexer.finish();
        errors.append(&mut self.errors);
        if self.has_type_function && errors.is_empty() {
            errors = check_type_functions(&chunk);
        }
        errors.sort_by_key(|e| e.span.start);
        let parse = ParseResult {
            source: self.source,
            chunk,
            comments,
            errors,
            nesting_depth: self.deepest_level,
            counts: self.counts,
        };
        (parse, self.tokens)
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

    #[inline]
    fn is_at_identifier(&self) -> bool {
        self.is_at(TokenKind::Identifier)
    }

    /// The current token's text, for a contextual keyword.
    #[inline]
    fn current_text(&self) -> &'ast str {
        self.token.span.text(self.source)
    }

    /// The next token's text, for a contextual keyword.
    fn next_text(&self) -> &'ast str {
        self.next.span.text(self.source)
    }

    /// Whether the current token is the identifier `text`.
    #[inline]
    fn is_at_name(&self, text: &str) -> bool {
        self.is_at_identifier() && self.current_text() == text
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

    /// Consume the current identifier, number, or string as a leaf. The
    /// caller has already matched its kind.
    #[inline]
    fn advance_leaf(&mut self) -> Literal<'ast> {
        let span = self.advance();
        Literal::new(span.text(self.source), span)
    }

    #[inline]
    fn advance_literal(&mut self) -> Box<'ast, Literal<'ast>> {
        let literal = self.advance_leaf();
        Box::new_in(literal, self.allocator)
    }

    /// Consume the current identifier as a name. The caller has already
    /// matched its kind.
    #[inline]
    fn advance_name(&mut self) -> Name<'ast> {
        let span = self.advance();
        let index = self.counts.names;
        self.counts.names = index.wrapping_add(1);
        Name::new(ThinStr::new(span.text(self.source)), Some(NameIndex::new(index)), span)
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
    /// holds it.
    #[inline]
    fn is_recovering(&self) -> bool {
        self.has_syntax_error
            || self
                .lexer
                .unterminated()
                .get(self.settled_unterminated)
                .is_some_and(|span| span.start < self.token.span.start)
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

    /// Close the bracket `opened` and consume its `kind` closer, as
    /// [`Self::expect`] does.
    #[inline]
    fn expect_closer(&mut self, kind: TokenKind, opened: Closer) -> Span {
        self.close(opened);
        if self.is_at(kind) { self.advance() } else { self.report_missing_closer(kind) }
    }

    /// A closing bracket of another kind that nothing open expects is a typo
    /// for the missing `kind`, so it closes the bracket too.
    #[cold]
    #[inline(never)]
    fn report_missing_closer(&mut self, kind: TokenKind) -> Span {
        let span = self.report_expected(kind);
        if matches!(
            self.peek(),
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
        ) && !self.is_at_expected_closer()
        {
            self.advance();
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

    /// Consume the current token if it's an identifier. Otherwise record an
    /// "expected identifier" error and leave the token in place.
    #[inline]
    fn expect_identifier(&mut self) -> Option<Name<'ast>> {
        if self.is_at_identifier() {
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
        if self.is_at_identifier() {
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
        Message: Into<String>,
    {
        self.errors.push(ParseError { span, message: message.into() });
    }

    /// Record a syntax error unless one is already being recovered from.
    #[cold]
    #[inline(never)]
    fn syntax_error<Message>(&mut self, span: Span, message: Message)
    where
        Message: Into<String>,
    {
        if !self.is_recovering() {
            self.has_syntax_error = true;
            self.error(span, message);
        }
    }

    /// Count one more level of nesting, or at [`MAX_NESTING_DEPTH`], stop
    /// the parse there and return false, leaving the count unchanged.
    #[must_use]
    #[inline]
    fn enter_depth(&mut self) -> bool {
        // A host-stack budget, independent of target VM compiler limits.
        if self.nesting_depth >= MAX_NESTING_DEPTH {
            self.stop_at_depth_limit();
            return false;
        }
        self.nesting_depth = self.nesting_depth.wrapping_add(1);
        self.deepest_level = self.deepest_level.max(self.nesting_depth);
        true
    }

    #[inline]
    const fn exit_depth(&mut self) {
        self.nesting_depth = self.nesting_depth.wrapping_sub(1);
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
    /// binary operators or suffixes, a cast, or a type operator. Everything in
    /// the operand moves one level down, so the cap applies to its deepest
    /// level, and stops the parse as [`Self::enter_depth`] does.
    ///
    /// On success the parser stands one level in, where the wrapper's other
    /// children go, and the caller steps back to `level` after them.
    #[must_use]
    #[inline]
    fn enter_wrapper(&mut self, level: usize) -> bool {
        if self.deepest_level >= MAX_NESTING_DEPTH {
            self.stop_at_depth_limit();
            return false;
        }
        self.deepest_level = self.deepest_level.wrapping_add(1);
        self.nesting_depth = level.wrapping_add(1);
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
        let start = self.token.span.start;
        self.has_syntax_error = false;
        self.settled_unterminated =
            self.lexer.unterminated().partition_point(|span| span.start < start);
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
            && (!matches!(kind, TokenKind::Identifier | TokenKind::LeftParen)
                || self.token.has_line_break_before)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::Path;

    use luck_ast::block_nesting_depth;
    use luck_testgen::{
        collect_fixture_files, detect_version, generate_full, generate_runtime_safe,
        is_error_fixture,
    };

    use super::*;

    /// The depth the parser counted and the depth of the tree it built.
    fn counted_and_built(source: &str, version: LuaVersion) -> (usize, usize) {
        let allocator = Allocator::new();
        let result = parse(&allocator, source, version);
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
                for source in
                    [generate_full(seed, version, 25), generate_runtime_safe(seed, version, 25)]
                {
                    let (counted, built) = counted_and_built(&source, version);
                    assert_eq!(counted, built, "seed {seed} {version:?}\n{source}");
                }
            }
        }
    }

    #[test]
    fn counts_the_depth_of_fixtures_as_the_tree_measures_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let files = collect_fixture_files(&root).expect("fixture tree is readable");
        let mut checked = 0usize;
        for path in files {
            let relative = path.strip_prefix(&root).expect("fixture is under the root");
            let is_source =
                matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau"));
            if !is_source || is_error_fixture(relative) {
                continue;
            }
            let source = fs::read_to_string(&path).expect("fixture is readable");
            let (counted, built) = counted_and_built(&source, detect_version(relative));
            assert_eq!(counted, built, "{}", path.display());
            checked = checked.saturating_add(1);
        }
        assert!(checked > 0, "no fixtures found under {}", root.display());
    }
}
