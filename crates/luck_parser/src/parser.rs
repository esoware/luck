mod attributes;
mod declaration;
mod expr;
mod function;
mod skip;
mod stmt;
mod types;

use core::mem;
use core::ops::Range;

use luck_ast::{BinaryLink, Block, Literal, MAX_NESTING_DEPTH, Suffix};
use luck_lexer::Lexer;
use luck_token::{Comment, CompactString, LuaVersion, Span, Token, TokenKind, has_line_break};
use rustc_hash::FxHashSet;

use crate::validate::check_type_functions;
use crate::{ParseError, ParseResult};

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

/// Lua needs only one token of lookahead, so `current` and `next` stand in
/// for a materialized token stream.
#[expect(clippy::struct_excessive_bools, reason = "each bool is a separate fact about the parse")]
struct Parser<'source> {
    lexer: Lexer<'source>,
    source: &'source str,
    current: Token,
    next: Token,
    previous_span: Span,
    version: LuaVersion,
    /// Levels open where the parser is, as [`MAX_NESTING_DEPTH`] counts them.
    nesting_depth: usize,
    /// The deepest level opened since the operand being parsed began. A node
    /// that wraps a finished operand nests all of it one level deeper, so the
    /// wrapper counts from here.
    deepest_level: usize,
    /// How many errors had been reported when the operand being parsed began.
    operand_first_error: usize,
    /// The chunk's own block is 1.
    block_depth: u32,
    /// The links of every binary run being parsed, innermost last. Each run
    /// takes its own off the end when it closes, so each gets one exact
    /// allocation however long it grows.
    binary_links: Vec<BinaryLink>,
    /// The suffix runs being parsed, kept like `binary_links`.
    suffixes: Vec<Suffix>,
    function_scope: FunctionScope,
    has_module_return: bool,
    has_value_exports: bool,
    exported_names: FxHashSet<CompactString>,
    /// Whether a `type function` appeared, so its body needs the scope check
    /// the parser can't do without tracking locals.
    has_type_function: bool,
    open: OpenConstructs,
    /// Whether a syntax error has been reported since the last statement
    /// began where one reliably does. What follows it in the same statement
    /// is in doubt, so its errors would only echo the first.
    is_recovering: bool,
    /// How many of the lexer's unterminated strings the parser has consumed.
    consumed_unterminated: usize,
    errors: Vec<ParseError>,
}

/// A token that closes a construct, or continues it, as `else` does an `if`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Closer {
    Paren,
    Bracket,
    Brace,
    /// An interpolated string's next segment, after an expression.
    StringSegment,
    End,
    /// `else` or `elseif`.
    Else,
    Until,
}

impl Closer {
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token closes nothing")]
    const fn of(kind: &TokenKind) -> Option<Self> {
        Some(match kind {
            TokenKind::RightParen => Self::Paren,
            TokenKind::RightBracket => Self::Bracket,
            TokenKind::RightBrace => Self::Brace,
            TokenKind::InterpolatedStringMiddle(_) | TokenKind::InterpolatedStringEnd(_) => {
                Self::StringSegment
            }
            TokenKind::End => Self::End,
            TokenKind::Else | TokenKind::ElseIf => Self::Else,
            TokenKind::Until => Self::Until,
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
    ends: u32,
    elses: u32,
    untils: u32,
}

impl OpenConstructs {
    const fn count_mut(&mut self, closer: Closer) -> &mut u32 {
        match closer {
            Closer::Paren => &mut self.parens,
            Closer::Bracket => &mut self.brackets,
            Closer::Brace => &mut self.braces,
            Closer::StringSegment => &mut self.string_segments,
            Closer::End => &mut self.ends,
            Closer::Else => &mut self.elses,
            Closer::Until => &mut self.untils,
        }
    }

    /// `end` and `until` each end any block, as in every Lua's parser, so a
    /// mistaken one ends the block it's in, and the construct around that
    /// block reports what it was missing.
    const fn expects(self, closer: Closer) -> bool {
        let count = match closer {
            Closer::Paren => self.parens,
            Closer::Bracket => self.brackets,
            Closer::Brace => self.braces,
            Closer::StringSegment => self.string_segments,
            Closer::End | Closer::Until => self.ends.saturating_add(self.untils),
            Closer::Else => self.elses,
        };
        count > 0
    }
}

/// What [`Parser::begin_operand`] saves of the operand around the one it
/// begins.
#[derive(Clone, Copy)]
struct OuterOperand {
    deepest_level: usize,
    first_error: usize,
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

impl<'source> Parser<'source> {
    /// # Errors
    ///
    /// Returns the lexer's refusal of a source too large for `u32` spans.
    fn new(source: &'source str, version: LuaVersion) -> Result<Self, ParseError> {
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
            deepest_level: 0,
            operand_first_error: 0,
            block_depth: 0,
            binary_links: Vec::new(),
            suffixes: Vec::new(),
            function_scope: FunctionScope { is_chunk: true, is_vararg: true, is_in_loop: false },
            has_module_return: false,
            has_value_exports: false,
            exported_names: FxHashSet::default(),
            has_type_function: false,
            open: OpenConstructs::default(),
            is_recovering: false,
            consumed_unterminated: 0,
            errors: Vec::new(),
        })
    }

    /// Nothing is open around the chunk's block, so it reads to the end of
    /// the source, and the lexer is drained by then.
    fn parse_chunk(mut self) -> (Block, Vec<Comment>, Vec<ParseError>) {
        let block = self.parse_block();
        let (comments, mut errors) = self.lexer.finish();
        errors.append(&mut self.errors);
        if self.has_type_function && errors.is_empty() {
            errors = check_type_functions(&block, self.version);
        }
        errors.sort_by_key(|e| e.span.start);
        (block, comments, errors)
    }

    /// Whether the source between the previous token and the current one
    /// contains a line break. Drives the 5.1/Luau "ambiguous syntax"
    /// check for `(` call suffixes.
    fn has_newline_before_current(&self) -> bool {
        let between = Span::new(self.previous_span.end, self.current.span.start);
        self.source
            .get(Range::<usize>::from(between))
            .is_some_and(|text| has_line_break(text, self.version))
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

    /// Consume the current token if it is a `kind`, whatever its payload.
    #[inline]
    fn eat(&mut self, kind: &TokenKind) -> bool {
        let is_at_kind = self.is_at(kind);
        if is_at_kind {
            self.advance_span();
        }
        is_at_kind
    }

    #[inline]
    fn advance(&mut self) -> Token {
        self.previous_span = self.current.span;
        if self.lexer.unterminated().get(self.consumed_unterminated) == Some(&self.current.span) {
            self.consume_unterminated();
        }
        mem::replace(&mut self.current, mem::replace(&mut self.next, self.lexer.next_token()))
    }

    /// A string cut off at its line break took what followed it on the line,
    /// often the closer of what holds it, and its lex error stands for what
    /// that leaves broken.
    #[cold]
    #[inline(never)]
    const fn consume_unterminated(&mut self) {
        self.consumed_unterminated = self.consumed_unterminated.saturating_add(1);
        self.is_recovering = true;
    }

    #[inline]
    fn advance_span(&mut self) -> Span {
        self.advance().span
    }

    /// Consume the current number or string token as a [`Literal`]. The
    /// caller has already matched its kind.
    fn advance_literal(&mut self) -> Literal {
        let token = self.advance();
        #[expect(clippy::unreachable, reason = "the caller has already matched this token kind")]
        let (TokenKind::Number(text) | TokenKind::Integer(text) | TokenKind::StringLiteral(text)) =
            token.kind
        else {
            unreachable!("caller matched a literal token kind");
        };
        Literal { text, span: token.span }
    }

    /// Consume a token of `kind` and return its span. On a mismatch, record
    /// an "expected X, found Y" error, leave the token in place, and return
    /// its span so parsing can go on.
    #[inline]
    fn expect(&mut self, kind: &TokenKind) -> Span {
        if self.is_at(kind) { self.advance_span() } else { self.report_expected(kind) }
    }

    /// Count a construct `closer` closes as open from here.
    const fn open(&mut self, closer: Closer) {
        let count = self.open.count_mut(closer);
        *count = count.saturating_add(1);
    }

    /// Count the construct `closer` closes as closed from here.
    const fn close(&mut self, closer: Closer) {
        let count = self.open.count_mut(closer);
        *count = count.saturating_sub(1);
    }

    /// Close the bracket `opened` and consume its `kind` closer, as
    /// [`Self::expect`] does.
    ///
    /// A closing bracket of another kind that nothing open expects is a typo
    /// for this one, so it closes this bracket too.
    fn expect_closer(&mut self, kind: &TokenKind, opened: Closer) -> Span {
        self.close(opened);
        if self.is_at(kind) {
            return self.advance_span();
        }
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
        if !self.is_recovering {
            let message = format!("expected {kind}, found {}", self.peek());
            self.syntax_error(span, message);
        }
        span
    }

    #[cold]
    #[inline(never)]
    fn report_expected_identifier(&mut self) {
        if !self.is_recovering {
            let span = self.current_span();
            let message = format!("expected identifier, found {}", self.peek());
            self.syntax_error(span, message);
        }
    }

    #[cold]
    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(ParseError { span, message: message.into() });
    }

    /// Record a syntax error unless one is already being recovered from,
    /// since what follows an error in the same statement is in doubt.
    #[cold]
    fn syntax_error(&mut self, span: Span, message: impl Into<String>) {
        if !self.is_recovering {
            self.is_recovering = true;
            self.error(span, message);
        }
    }

    /// Whether `levels` more levels of nesting fit where the parser is.
    ///
    /// A construct that doesn't fit is refused: the parser reports it once,
    /// skips its tokens without parsing them, and puts an error node where it
    /// was. So no tree the parser returns, errors or not, nests deeper than
    /// [`MAX_NESTING_DEPTH`], and recursion stops at the cap.
    const fn can_nest(&self, levels: usize) -> bool {
        // A host-stack budget, independent of target VM compiler limits.
        self.nesting_depth.saturating_add(levels) <= MAX_NESTING_DEPTH
    }

    /// Count one more level of nesting, which the caller has checked fits.
    fn enter_depth(&mut self) {
        debug_assert!(self.can_nest(1), "a level was entered past the cap");
        self.nesting_depth = self.nesting_depth.saturating_add(1);
        self.deepest_level = self.deepest_level.max(self.nesting_depth);
    }

    const fn exit_depth(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
    }

    #[cold]
    #[inline(never)]
    fn report_depth_limit(&mut self, span: Span) {
        self.error(
            span,
            format!(
                "maximum nesting depth exceeded (luck parser resource limit: {MAX_NESTING_DEPTH})"
            ),
        );
    }

    /// Refuse the bracket or interpolated string the current token opens,
    /// returning the span it covered.
    #[cold]
    #[inline(never)]
    fn refuse_group(&mut self) -> Span {
        let open = self.current_span();
        self.report_depth_limit(open);
        self.skip_nested();
        open.merge(self.previous_span)
    }

    /// Start measuring an operand at the current level. Hand the result to
    /// [`Self::end_operand`] once the operand is whole.
    const fn begin_operand(&mut self) -> OuterOperand {
        OuterOperand {
            deepest_level: mem::replace(&mut self.deepest_level, self.nesting_depth),
            first_error: mem::replace(&mut self.operand_first_error, self.errors.len()),
        }
    }

    /// Fold the finished operand's depth into that of the operand around it.
    fn end_operand(&mut self, outer: OuterOperand) {
        self.deepest_level = self.deepest_level.max(outer.deepest_level);
        self.operand_first_error = outer.first_error;
    }

    /// Count a node that wraps the operand parsed so far at `level`: a run of
    /// binary operators or suffixes, a cast, or a type operator. Everything in
    /// the operand moves one level down, so the cap applies to its deepest
    /// level, and false counts nothing.
    ///
    /// On success the parser stands one level in, where the wrapper's other
    /// children go, and the caller steps back to `level` after them.
    #[must_use]
    const fn enter_wrapper(&mut self, level: usize) -> bool {
        if self.deepest_level >= MAX_NESTING_DEPTH {
            return false;
        }
        self.deepest_level = self.deepest_level.saturating_add(1);
        self.nesting_depth = level.saturating_add(1);
        true
    }

    /// Report a wrapper refused at the operator that opens it, then count the
    /// error node that will replace the wrapper and its operand as a leaf at
    /// `level`.
    ///
    /// An operand that already holds an error, often the refusal that cut it
    /// off at the cap, has had its report, so the wrapper adds none.
    fn refuse_wrapper(&mut self, level: usize) {
        if self.errors.len() == self.operand_first_error {
            self.report_depth_limit(self.current_span());
        }
        self.deepest_level = level;
    }

    /// Split the leading `>` off the current two-character token, leaving
    /// `remainder` as the current token, and return the `>`'s span.
    fn split_close_angle(&mut self, remainder: TokenKind) -> Span {
        let span = self.current.span;
        let split_at = span.start.saturating_add(1);
        self.current = Token::new(remainder, Span::new(split_at, span.end));
        self.previous_span = Span::new(span.start, split_at);
        self.previous_span
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

    fn is_at_reliable_stmt_start(&self) -> bool {
        let kind = self.peek();
        stmt::is_stmt_start(kind)
            && (!matches!(kind, TokenKind::Identifier(_) | TokenKind::LeftParen)
                || self.has_newline_before_current())
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
        let mut parser = Parser::new(source, version).expect("source fits in u32 spans");
        let block = parser.parse_block();
        assert!(parser.errors.is_empty(), "{:?}\n{source}", parser.errors.first());
        (parser.deepest_level, block_nesting_depth(&block))
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
