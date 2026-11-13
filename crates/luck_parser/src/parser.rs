mod attributes;
mod declaration;
mod expr;
mod function;
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
    errors: Vec<ParseError>,
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
            block_depth: 0,
            binary_links: Vec::new(),
            suffixes: Vec::new(),
            function_scope: FunctionScope { is_chunk: true, is_vararg: true, is_in_loop: false },
            has_module_return: false,
            has_value_exports: false,
            exported_names: FxHashSet::default(),
            has_type_function: false,
            errors: Vec::new(),
        })
    }

    fn parse_chunk(mut self) -> (Block, Vec<Comment>, Vec<ParseError>) {
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
        mem::replace(&mut self.current, mem::replace(&mut self.next, self.lexer.next_token()))
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
        let (TokenKind::Number(text) | TokenKind::StringLiteral(text)) = token.kind else {
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
            self.report_depth_limit();
            return false;
        }
        self.nesting_depth = self.nesting_depth.saturating_add(1);
        self.deepest_level = self.deepest_level.max(self.nesting_depth);
        true
    }

    const fn exit_depth(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
    }

    #[cold]
    #[inline(never)]
    fn report_depth_limit(&mut self) {
        let span = self.current_span();
        self.error(
            span,
            format!(
                "maximum nesting depth exceeded (luck parser resource limit: {MAX_NESTING_DEPTH})"
            ),
        );
    }

    /// Start measuring an operand at the current level. Hand the result to
    /// [`Self::end_operand`] once the operand is whole.
    const fn begin_operand(&mut self) -> usize {
        mem::replace(&mut self.deepest_level, self.nesting_depth)
    }

    /// Fold the finished operand's depth into that of the operand around it.
    fn end_operand(&mut self, outer_deepest_level: usize) {
        self.deepest_level = self.deepest_level.max(outer_deepest_level);
    }

    /// Count a node that wraps the operand parsed so far at `level`: a run of
    /// binary operators or suffixes, a cast, or a type operator. Everything in
    /// the operand moves one level down, so the cap applies to its deepest
    /// level.
    ///
    /// On success the parser stands one level in, where the wrapper's other
    /// children go, and the caller steps back to `level` after them.
    #[must_use]
    fn enter_wrapper(&mut self, level: usize) -> bool {
        self.nesting_depth = self.deepest_level;
        let has_entered = self.enter_depth();
        self.nesting_depth = if has_entered { level.saturating_add(1) } else { level };
        has_entered
    }

    /// Skip to the next statement start, block end, or closing bracket.
    /// Always consumes at least one token, so recovery can't loop forever.
    fn synchronize(&mut self) {
        self.advance();
        while !self.is_at_eof() {
            if stmt::is_stmt_start(self.peek())
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
