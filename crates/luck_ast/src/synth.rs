mod identifier;

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

pub use identifier::{
    is_valid_identifier, is_valid_identifier_in, sanitize_identifier, sanitize_identifier_in,
};
use luck_token::{
    BinOp, CompactString, CompoundOp, LuaVersion, OperandSide, Span, Token, TokenKind, UnOp,
};

use crate::expr::{
    BinaryOp, ElseIfExprClause, Expression, FieldAccess, FunctionArgs, FunctionCall, FunctionDef,
    IfExpression, IndexExpression, InterpolatedString, InterpolatedStringSegment, Literal,
    ParenthesizedExpression, TableConstructor, TypeCast, TypeInstantiation, UnaryOp, Var,
};
use crate::query::is_prefix_expression;
use crate::shared::{Block, Field, FunctionBody, Parameter, Punctuated, VarArgParam};
use crate::stmt::{
    Assignment, Attribute, AttributedName, CompoundAssignment, DoBlock, ElseClause, ElseIfClause,
    FunctionAttribute, FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction,
    GlobalStar, GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment,
    LocalFunction, NumericFor, RepeatLoop, ReturnStatement, Statement, TypeDeclaration,
    TypeDeclarationValue, WhileLoop,
};
use crate::types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypePack, TypeofType, UnionType, VariadicType,
};

/// Node constructors backed by a monotonic span counter.
///
/// Builds `luck_ast` nodes without any source text. Writing the node structs
/// by hand means spelling out every `Token { kind, span }`; these
/// constructors do that for you. They're built for tools that emit an AST
/// directly, so they also take on the output-validity work such a tool would
/// otherwise reimplement.
///
/// Methods take `&self`, so calls nest:
/// `synth.call(synth.name_expr("f"), vec![synth.nil()])`.
///
/// # Spans
///
/// Every synthesized node gets a fresh single-point span `Span::new(n, n)`,
/// with `n` strictly increasing until the counter saturates at `u32::MAX`,
/// after which every later node gets that offset instead of wrapping back to
/// zero. These spans don't index into any source; they only keep nodes
/// distinguishable for comment anchoring and diagnostics. Spans are unique
/// within one `Synth` and across every synthesizer created from it with
/// [`Synth::share`], so when several synthesizers feed one tree (parallel
/// decompilation of function protos, say), have them share one counter. To
/// splice synthetic nodes into a parsed AST, use [`Synth::starting_at`] to
/// put them above the source's byte offsets, or comment anchors will
/// collide.
///
/// # What the constructors guarantee
///
/// - **Operator precedence.** [`Synth::binary_op`], [`Synth::unary_op`], and
///   [`Synth::type_cast`] parenthesize their operands whenever the printed
///   form would re-parse differently: precedence and associativity for
///   nested binary operators, `(-a)^b`, greedy Luau if-expressions,
///   `(a + b) :: T`, chained casts (`(a :: T) :: U`), and casts printed
///   before `<`, `&`, or `|`, which the type grammar would otherwise
///   swallow (`(a :: T) < b`).
/// - **Type composition.** [`Synth::ty_optional`], [`Synth::ty_union`], and
///   [`Synth::ty_intersection`] parenthesize union, intersection, and
///   function members. That keeps their meaning and respects Luau's ban on
///   mixing `|` and `&` (or `?` and `&`) without parentheses.
/// - **Prefix positions.** Call callees, method receivers, and index and
///   field prefixes are wrapped unless they're already a var, call,
///   parenthesized expression, or type instantiation: `("s"):rep(2)`,
///   `({}).x`, `(function() end)()`.
/// - **Escaping.** [`Synth::string`] and [`Synth::string_bytes`] produce the
///   quoted token text. Decimal escapes are always three digits, so a
///   following literal digit can't extend them. Interpolated-string segments
///   escape `` ` ``, `{`, and `\`. [`Synth::long_string`] falls back to the
///   quoted form for carriage returns and NUL bytes.
/// - **Numeric literals.** [`Synth::number_f64`] and [`Synth::number_i64`]
///   render any value, including those with no literal form: negatives
///   (unary minus node), infinities (`1/0`), NaN (`0/0`), and `i64::MIN`
///   (hex, which wraps to the intended integer on Lua 5.3+). Integral floats
///   get a `.0` suffix so they stay floats on 5.3+, and exponent form
///   replaces the plain one whenever it's shorter (`1e300`). On a version
///   pinned to a single-number dialect (5.1, 5.2, Luau), integral floats
///   drop the `.0`, `i64::MIN` becomes an exact negated decimal literal, and
///   other integers may round to the nearest double. NaN sign and payload
///   aren't preserved.
/// - **Loud failure on invalid names.** [`Synth::identifier`] asserts, in release
///   builds too, that its argument is an identifier, so hostile input such
///   as bytecode debug info or obfuscated names fails fast instead of
///   producing output that won't re-parse.
///
/// # What the caller still owes
///
/// - **Multi-value truncation.** `f(g())` spreads all of `g`'s results.
///   Wrap the expression in [`Synth::single_value`] when exactly one value is
///   meant at the end of a call, return, or assignment list. Only the caller
///   knows which positions need it.
/// - **Version gating.** Nothing stops you from synthesizing, say, a Lua 5.4
///   attribute for a Luau target, so pick constructors that match the
///   dialect. [`Synth::with_version`] pins name validity, field-name safety,
///   and number rendering to one dialect, but deliberately doesn't gate
///   constructs.
/// - **Identifier validity.** For names that may not be identifiers (such as
///   bytecode debug info), build fields with [`Synth::field_or_index`] and
///   table keys with [`Synth::record`], which fall back to the bracketed
///   string form. Other name positions (locals, params, labels, function and
///   method names) have no bracketed spelling, so rewrite those names with
///   [`sanitize_identifier`] / [`sanitize_identifier_in`], or check them with
///   [`is_valid_identifier`] / [`is_valid_identifier_in`] and rename them
///   yourself. Sanitized names are deterministic but can collide, so
///   deduplication is up to the caller.
///
/// To format the result, pass it to `luck_formatter::format_block`, with any
/// [`SyntheticComment`]s wrapped in `Comments::synthetic`. For compact
/// output, call `luck_codegen::compact`.
///
/// # Usage
///
/// ```
/// use luck_ast::Synth;
/// use luck_token::BinOp;
///
/// let synth = Synth::new();
/// // total = a + b * 2 -- precedence handled, no manual parens.
/// let sum = synth.binary_op(
///     synth.name_expr("a"),
///     BinOp::Add,
///     synth.binary_op(synth.name_expr("b"), BinOp::Mul, synth.number_i64(2)),
/// );
/// let stmt = synth.assign(vec![synth.name_var("total")], vec![sum]);
/// let block = synth.block(vec![stmt], None);
/// assert_eq!(block.stmts.len(), 1);
/// ```
#[derive(Debug, Default)]
pub struct Synth {
    next_offset: Arc<AtomicU32>,
    version: Option<LuaVersion>,
}

/// A table field for [`Synth::table`], which fills in the spans and
/// separators a real [`Field`] needs.
#[derive(Debug)]
pub enum FieldSpec<'name> {
    Positional(Expression),
    Named(&'name str, Expression),
    Bracketed(Expression, Expression),
}

/// One piece of a [`Synth::interpolated_string`]: unescaped literal text or
/// an interpolated `{expr}`.
#[derive(Debug)]
pub enum InterpolatedStringPart<'text> {
    Text(&'text str),
    Expr(Expression),
}

/// A table-type field for [`Synth::ty_table`], which fills in the spans and
/// separators a real [`TypeField`] needs.
#[derive(Debug)]
pub enum TypeFieldSpec<'name> {
    Named { access: Option<TypeFieldAccess>, name: &'name str, value: Type },
    Indexer { access: Option<TypeFieldAccess>, key: Type, value: Type },
    Array(Type),
}

/// Luau `read`/`write` access modifier on a table-type field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFieldAccess {
    Read,
    Write,
}

/// A full function signature. `Default` is an empty, untyped `()`, so set
/// only the fields you need.
#[derive(Debug, Default)]
pub struct FunctionSignature {
    /// Luau `<T, U...>` list before the parameter parens.
    pub generics: Option<GenericTypeList>,
    pub params: Vec<Parameter>,
    /// Trailing `...`, optionally typed. See [`Synth::vararg_param`].
    pub vararg: Option<VarArgParam>,
    /// Luau `: T` return annotation.
    pub return_type: Option<Type>,
}

/// A comment for the formatter to attach to a synthesized node.
///
/// Comments live outside the AST, keyed by `attached_to` (the anchor node's
/// span start), so this is plain data that only the formatter interprets.
/// The formatter adds `-- ` when `text` doesn't already start with `--`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticComment {
    pub attached_to: u32,
    pub text: CompactString,
    pub is_leading: bool,
}

impl SyntheticComment {
    /// A comment printed on its own line before `stmt`.
    #[must_use]
    pub fn leading(stmt: &Statement, text: &str) -> Self {
        Self { attached_to: stmt.span().start, text: CompactString::from(text), is_leading: true }
    }

    /// A comment printed after `stmt` on the same line.
    #[must_use]
    pub fn trailing(stmt: &Statement, text: &str) -> Self {
        Self { attached_to: stmt.span().start, text: CompactString::from(text), is_leading: false }
    }

    /// A comment printed inside `block` when it's an empty function body,
    /// such as `-- unreachable`.
    #[must_use]
    pub fn dangling(block: &Block, text: &str) -> Self {
        Self { attached_to: block.span.start, text: CompactString::from(text), is_leading: true }
    }
}

/// Whether the type grammar consumes this operator's token when it directly
/// follows a printed type: `<` opens generic arguments after a named type,
/// `|` and `&` continue a union or intersection.
const fn op_token_extends_types(op: BinOp) -> bool {
    match op {
        BinOp::Lt | BinOp::BitAnd | BinOp::BitOr => true,
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::Concat
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => false,
    }
}

/// Whether `expr` prints with a type cast as its rightmost part, so the next
/// token lands directly after the cast's type.
fn ends_with_type_cast(expr: &Expression) -> bool {
    match expr {
        // Luau
        Expression::TypeCast(_) => true,
        Expression::BinaryOp(inner) => ends_with_type_cast(&inner.right),
        Expression::UnaryOp(inner) => ends_with_type_cast(&inner.operand),
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}

impl Synth {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A synthesizer whose spans start at `offset`, for splicing nodes into a
    /// parsed AST without colliding with its byte offsets. For several
    /// synthesizers feeding one tree, use [`Synth::share`] instead of picking
    /// disjoint ranges.
    #[must_use]
    pub fn starting_at(offset: u32) -> Self {
        Self { next_offset: Arc::new(AtomicU32::new(offset)), version: None }
    }

    /// A synthesizer that shares this one's span counter and version pin.
    /// Spans never collide across the family, so give one to each thread when
    /// synthesizing in parallel.
    #[must_use]
    pub fn share(&self) -> Self {
        Self { next_offset: Arc::clone(&self.next_offset), version: self.version }
    }

    /// Pins name handling and number rendering to one dialect.
    ///
    /// [`Synth::identifier`] checks names against that dialect's keywords
    /// instead of only the ones every dialect reserves. [`Synth::field_or_index`]
    /// and [`Synth::record`] use the dot or named form for any name that's an
    /// identifier there (`t.goto` on Luau) instead of falling back to
    /// brackets. [`Synth::number_f64`] and [`Synth::number_i64`] render for
    /// that dialect's number model. It deliberately doesn't gate which
    /// constructs you can build.
    #[must_use]
    pub const fn with_version(mut self, version: LuaVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// A fresh single-point span, distinct from every earlier one until the
    /// counter saturates.
    fn next_span(&self) -> Span {
        // Relaxed is enough: uniqueness needs atomicity, not ordering. The
        // loop stops at `u32::MAX` instead of wrapping, since a wrapped
        // counter would hand out offsets that collide with earlier nodes or
        // real source positions. Repeating `u32::MAX` keeps synthesis
        // infallible and only costs comment placement.
        let mut offset = self.next_offset.load(Ordering::Relaxed);
        while let Some(next) = offset.checked_add(1) {
            match self.next_offset.compare_exchange_weak(
                offset,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => offset = observed,
            }
        }
        Span::new(offset, offset)
    }

    #[must_use]
    pub fn token(&self, kind: TokenKind) -> Token {
        Token::new(kind, self.next_span())
    }

    /// An identifier token.
    ///
    /// Asserts validity in release builds too, so an untrusted name fails
    /// loudly instead of producing output that won't re-parse. Names that may
    /// not be identifiers belong in [`Synth::field_or_index`],
    /// [`Synth::record`], or [`Synth::string`], or need
    /// [`sanitize_identifier`] / [`sanitize_identifier_in`] first.
    #[must_use]
    pub fn identifier(&self, name: &str) -> Token {
        self.token(TokenKind::Identifier(self.identifier_name(name)))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't an identifier under the pinned version, or in
    /// any dialect when none is pinned.
    fn identifier_name(&self, name: &str) -> CompactString {
        let is_valid = self.version.map_or_else(
            || is_valid_identifier(name),
            |version| is_valid_identifier_in(name, version),
        );
        assert!(is_valid, "synth: {name:?} is not a valid identifier");
        CompactString::from(name)
    }

    /// Whether `name` works as a `.name` field or `name =` key: an identifier
    /// in the pinned dialect, or in every dialect when none is pinned.
    fn is_safe_field_name(&self, name: &str) -> bool {
        self.version.map_or_else(
            || LuaVersion::ALL.iter().all(|&version| is_valid_identifier_in(name, version)),
            |version| is_valid_identifier_in(name, version),
        )
    }

    #[must_use]
    pub fn name_var(&self, name: &str) -> Var {
        Var::Name(self.identifier(name))
    }

    #[must_use]
    pub fn index_var(&self, prefix: Expression, index: Expression) -> Var {
        Var::Index(Box::new(IndexExpression {
            span: self.next_span(),
            prefix: self.wrap_prefix(prefix),
            index,
        }))
    }

    #[must_use]
    pub fn field_var(&self, prefix: Expression, name: &str) -> Var {
        Var::FieldAccess(Box::new(FieldAccess {
            span: self.next_span(),
            prefix: self.wrap_prefix(prefix),
            name: self.identifier(name),
        }))
    }

    #[must_use]
    pub fn name_expr(&self, name: &str) -> Expression {
        Expression::Var(self.name_var(name))
    }

    #[must_use]
    pub fn index(&self, prefix: Expression, index: Expression) -> Expression {
        Expression::Var(self.index_var(prefix, index))
    }

    #[must_use]
    pub fn field(&self, prefix: Expression, name: &str) -> Expression {
        Expression::Var(self.field_var(prefix, name))
    }

    /// `prefix.name` when `name` is a safe identifier (in the pinned dialect,
    /// or in every dialect when none is pinned), `prefix["name"]` otherwise.
    #[must_use]
    pub fn field_or_index(&self, prefix: Expression, name: &str) -> Expression {
        if self.is_safe_field_name(name) {
            self.field(prefix, name)
        } else {
            let key = self.string(name);
            self.index(prefix, key)
        }
    }

    /// A number from already-formatted literal text. Prefer
    /// [`Synth::number_f64`] / [`Synth::number_i64`] for runtime values.
    #[must_use]
    pub fn number(&self, text: &str) -> Expression {
        Expression::Number(Literal { text: CompactString::from(text), span: self.next_span() })
    }

    /// A Luau integer literal from source-form text including the `i` suffix.
    ///
    /// # Panics
    ///
    /// Panics if `text` doesn't end with `i`.
    #[must_use]
    pub fn integer_literal(&self, text: &str) -> Expression {
        assert!(text.ends_with('i'), "synth: Luau integer literal must end with 'i'");
        Expression::Integer(Literal { text: CompactString::from(text), span: self.next_span() })
    }

    /// An exact Luau 64-bit integer. A literal can't carry a sign, so
    /// negative values use their two's-complement hex form.
    #[must_use]
    pub fn integer_i64(&self, value: i64) -> Expression {
        if value < 0 {
            self.integer_literal(&format!("0x{value:x}i"))
        } else {
            self.integer_literal(&format!("{value}i"))
        }
    }

    /// An `f64` as a numeric expression, assuming the target uses IEEE-754
    /// doubles.
    ///
    /// Finite values and infinities round-trip, signed zero included. NaN
    /// becomes `0/0` and deliberately loses its sign, payload, and signaling
    /// state, since only a target-specific runtime construction can rebuild
    /// a bit-exact NaN.
    ///
    /// Integral floats get a `.0` suffix so they stay floats on Lua 5.3+,
    /// unless the version is pinned to a single-number dialect (5.1, 5.2,
    /// Luau). Exponent notation replaces the plain form when it's shorter.
    /// Negatives use unary minus and infinities use `1/0`.
    #[must_use]
    pub fn number_f64(&self, value: f64) -> Expression {
        if value.is_nan() {
            return self.binary_op(self.number("0"), BinOp::Div, self.number("0"));
        }
        if value.is_infinite() {
            let infinity = self.binary_op(self.number("1"), BinOp::Div, self.number("0"));
            return if value < 0.0 { self.unary_op(UnOp::Neg, infinity) } else { infinity };
        }
        if value.is_sign_negative() {
            // Lua has no negative literals; this also keeps -0.0's sign bit.
            return self.unary_op(UnOp::Neg, self.number_f64(value.abs()));
        }
        // `Display` never uses exponent notation, so a missing `.` means the
        // value is integral.
        let mut text = value.to_string();
        let should_mark_float_subtype = self.version.is_none_or(LuaVersion::has_integer_subtype);
        if !text.contains('.') && should_mark_float_subtype {
            text.push_str(".0");
        }
        // Both forms carry shortest round-trip digits; an exponent implies
        // the float subtype on 5.3+ just as `.0` does.
        let exponent = format!("{value:e}");
        if exponent.len() < text.len() {
            text = exponent;
        }
        self.number(&text)
    }

    /// An `i64` as an ordinary Lua number.
    ///
    /// Exact on Lua 5.3+ with 64-bit integers, which is also what an
    /// unpinned synthesizer assumes. On single-number dialects (5.1, 5.2,
    /// Luau), integers outside `[-2^53, 2^53]` can round to the nearest
    /// double. For Luau's separate exact integer type, use
    /// [`Synth::integer_i64`].
    #[must_use]
    pub fn number_i64(&self, value: i64) -> Expression {
        if value == i64::MIN {
            // On 5.3+ the decimal magnitude overflows into a float, while
            // hex wraps to the intended integer. On a single-number dialect
            // hex reads as +2^63, but the decimal magnitude is an exact
            // double, so negating it works there.
            if self.version.is_some_and(|version| !version.has_integer_subtype()) {
                return self.unary_op(UnOp::Neg, self.number("9223372036854775808"));
            }
            return self.number("0x8000000000000000");
        }
        if value < 0 {
            return self.unary_op(UnOp::Neg, self.number(&value.unsigned_abs().to_string()));
        }
        self.number(&value.to_string())
    }

    /// A string literal from unescaped `content`, stored as escaped,
    /// double-quoted token text.
    #[must_use]
    pub fn string(&self, content: &str) -> Expression {
        Expression::StringLiteral(Literal { text: quote_utf8(content), span: self.next_span() })
    }

    /// A string literal from arbitrary bytes, such as a bytecode constant
    /// that isn't UTF-8. Bytes outside printable ASCII are escaped, so the
    /// token text is always valid UTF-8.
    #[must_use]
    pub fn string_bytes(&self, content: &[u8]) -> Expression {
        Expression::StringLiteral(Literal { text: quote_bytes(content), span: self.next_span() })
    }

    /// A `[[...]]` long-bracket string at the lowest `=` level whose closer
    /// neither appears in the content nor straddles its end. Content with
    /// carriage returns (which Lua normalizes) or NUL bytes (which Luau
    /// rejects) falls back to [`Synth::string`].
    #[must_use]
    pub fn long_string(&self, content: &str) -> Expression {
        if content.contains(['\r', '\0']) {
            return self.string(content);
        }
        let mut equals = String::new();
        // The closer's first byte can finish a delimiter begun in content.
        while content.contains(&format!("]{equals}]")) || content.ends_with(&format!("]{equals}")) {
            equals.push('=');
        }
        // Lua swallows a newline directly after the opening bracket.
        let leading_newline = if content.starts_with('\n') { "\n" } else { "" };
        Expression::StringLiteral(Literal {
            text: CompactString::from(format!("[{equals}[{leading_newline}{content}]{equals}]")),
            span: self.next_span(),
        })
    }

    #[must_use]
    pub fn nil(&self) -> Expression {
        Expression::Nil(self.next_span())
    }

    #[must_use]
    pub fn boolean(&self, value: bool) -> Expression {
        if value { Expression::True(self.next_span()) } else { Expression::False(self.next_span()) }
    }

    #[must_use]
    pub fn vararg(&self) -> Expression {
        Expression::VarArg(self.next_span())
    }

    /// `lhs op rhs`, parenthesizing either operand where precedence,
    /// associativity, a greedy if-expression, or a type cast printed before
    /// `<`, `&`, or `|` would otherwise change the parse.
    #[must_use]
    pub fn binary_op(&self, lhs: Expression, op: BinOp, rhs: Expression) -> Expression {
        let left = self.wrap_binary_operand(lhs, op, OperandSide::Left);
        let right = self.wrap_binary_operand(rhs, op, OperandSide::Right);
        Expression::BinaryOp(Box::new(BinaryOp { span: self.next_span(), left, op, right }))
    }

    /// `op operand`, parenthesizing a binary or if-expression operand.
    #[must_use]
    pub fn unary_op(&self, op: UnOp, operand: Expression) -> Expression {
        let wrapped = match operand {
            Expression::BinaryOp(_) | Expression::IfExpression(_) => self.parenthesized(operand),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::UnaryOp(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => operand,
        };
        Expression::UnaryOp(Box::new(UnaryOp { span: self.next_span(), op, operand: wrapped }))
    }

    #[must_use]
    pub fn parenthesized(&self, expr: Expression) -> Expression {
        Expression::Parenthesized(Box::new(ParenthesizedExpression {
            span: self.next_span(),
            expr,
        }))
    }

    /// Truncates a call or `...`, the only multi-value forms, to one value by
    /// parenthesizing it. Anything else passes through unchanged.
    ///
    /// Use it at the end of a call, return, or assignment list when exactly
    /// one value is meant, such as a bytecode `CALL` that requests one result.
    #[must_use]
    pub fn single_value(&self, expr: Expression) -> Expression {
        match expr {
            Expression::FunctionCall(_) | Expression::VarArg(_) => self.parenthesized(expr),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => expr,
        }
    }

    /// Luau type cast: `expr :: T`. A cast binds to a simple expression, so
    /// binary, unary, and if-expression operands get parentheses, and so do
    /// cast operands, since Luau rejects `a :: T :: U`.
    #[must_use]
    pub fn type_cast(&self, expr: Expression, type_value: Type) -> Expression {
        let wrapped = match expr {
            Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_) => self.parenthesized(expr),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => expr,
        };
        Expression::TypeCast(Box::new(TypeCast {
            span: self.next_span(),
            expr: wrapped,
            type_annotation: type_value,
        }))
    }

    /// Luau explicit type-parameter instantiation: `expr<<T, U>>`.
    #[must_use]
    pub fn type_instantiation(&self, expr: Expression, type_args: Vec<Type>) -> Expression {
        Expression::TypeInstantiation(Box::new(TypeInstantiation {
            span: self.next_span(),
            expr: self.wrap_prefix(expr),
            type_args: TypeArgs { span: self.next_span(), args: Punctuated::from_items(type_args) },
        }))
    }

    fn wrap_binary_operand(
        &self,
        operand: Expression,
        parent_op: BinOp,
        side: OperandSide,
    ) -> Expression {
        let should_parenthesize = match &operand {
            Expression::BinaryOp(inner) => !inner.op.can_nest_bare(parent_op, side),
            // Only `^` binds tighter than unary; `a ^ -b` needs no parens
            // because a unary operator is always accepted after a binop.
            Expression::UnaryOp(_) => {
                side == OperandSide::Left && parent_op.precedence().0 > UnOp::PRECEDENCE
            }
            // Luau: an if-expression extends as far right as it can, so
            // anything printed after it lands in its else branch. The right
            // side gets wrapped too, since a later composition may print
            // after it.
            Expression::IfExpression(_) => true,
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => false,
        } || (side == OperandSide::Left
            && op_token_extends_types(parent_op)
            && ends_with_type_cast(&operand));
        if should_parenthesize { self.parenthesized(operand) } else { operand }
    }

    /// Parenthesizes an expression that can't stand in prefix position.
    fn wrap_prefix(&self, prefix: Expression) -> Expression {
        if is_prefix_expression(&prefix) { prefix } else { self.parenthesized(prefix) }
    }

    #[must_use]
    pub fn call(&self, callee: Expression, args: Vec<Expression>) -> Expression {
        self.call_node(callee, None, None, self.parenthesized_args(args))
    }

    #[must_use]
    pub fn method_call(
        &self,
        receiver: Expression,
        name: &str,
        args: Vec<Expression>,
    ) -> Expression {
        self.call_node(receiver, Some(name), None, self.parenthesized_args(args))
    }

    /// `receiver:name<<T, U>>(args)`.
    #[must_use]
    pub fn method_call_with_types(
        &self,
        receiver: Expression,
        name: &str,
        type_args: Vec<Type>,
        args: Vec<Expression>,
    ) -> Expression {
        self.call_node(receiver, Some(name), Some(type_args), self.parenthesized_args(args))
    }

    /// `callee{ fields }`, the table-constructor argument form.
    #[must_use]
    pub fn call_table(&self, callee: Expression, fields: Vec<FieldSpec<'_>>) -> Expression {
        let args = FunctionArgs::TableConstructor(Box::new(self.table_constructor(fields)));
        self.call_node(callee, None, None, args)
    }

    /// `callee"content"`, the string argument form (content is unquoted).
    #[must_use]
    pub fn call_string(&self, callee: Expression, content: &str) -> Expression {
        let args = FunctionArgs::StringLiteral(Literal {
            text: quote_utf8(content),
            span: self.next_span(),
        });
        self.call_node(callee, None, None, args)
    }

    /// `receiver:name{ fields }`.
    #[must_use]
    pub fn method_call_table(
        &self,
        receiver: Expression,
        name: &str,
        fields: Vec<FieldSpec<'_>>,
    ) -> Expression {
        let args = FunctionArgs::TableConstructor(Box::new(self.table_constructor(fields)));
        self.call_node(receiver, Some(name), None, args)
    }

    /// `receiver:name"content"`.
    #[must_use]
    pub fn method_call_string(
        &self,
        receiver: Expression,
        name: &str,
        content: &str,
    ) -> Expression {
        let args = FunctionArgs::StringLiteral(Literal {
            text: quote_utf8(content),
            span: self.next_span(),
        });
        self.call_node(receiver, Some(name), None, args)
    }

    fn call_node(
        &self,
        callee: Expression,
        method: Option<&str>,
        type_args: Option<Vec<Type>>,
        args: FunctionArgs,
    ) -> Expression {
        Expression::FunctionCall(Box::new(FunctionCall {
            span: self.next_span(),
            callee: self.wrap_prefix(callee),
            args,
            method: method.map(|name| self.identifier(name)),
            explicit_type_args: type_args.map(|types| {
                Box::new(TypeArgs { span: self.next_span(), args: Punctuated::from_items(types) })
            }),
        }))
    }

    #[must_use]
    pub fn table(&self, fields: Vec<FieldSpec<'_>>) -> Expression {
        Expression::TableConstructor(Box::new(self.table_constructor(fields)))
    }

    /// `{ v1, v2, ... }`, an all-positional table.
    #[must_use]
    pub fn array(&self, values: Vec<Expression>) -> Expression {
        self.table(values.into_iter().map(FieldSpec::Positional).collect())
    }

    /// `{ k = v, ... }`, a named-field table. Keys that aren't safe
    /// identifiers fall back to the bracketed string form.
    #[must_use]
    pub fn record(&self, fields: Vec<(&str, Expression)>) -> Expression {
        self.table(
            fields
                .into_iter()
                .map(|(name, value)| {
                    if self.is_safe_field_name(name) {
                        FieldSpec::Named(name, value)
                    } else {
                        FieldSpec::Bracketed(self.string(name), value)
                    }
                })
                .collect(),
        )
    }

    fn table_constructor(&self, fields: Vec<FieldSpec<'_>>) -> TableConstructor {
        let span = self.next_span();
        let built_fields = fields
            .into_iter()
            .map(|field| match field {
                FieldSpec::Positional(value) => Field::Positional { span: self.next_span(), value },
                FieldSpec::Named(name, value) => {
                    Field::Named { span: self.next_span(), name: self.identifier(name), value }
                }
                FieldSpec::Bracketed(key, value) => {
                    Field::Bracketed { span: self.next_span(), key, value }
                }
            })
            .collect();
        TableConstructor { span, fields: Punctuated::from_items(built_fields) }
    }

    /// `function(params) body end` with plain untyped parameters. For
    /// attributes, generics, typed parameters, varargs, or a return type,
    /// use [`Synth::function_def_full`].
    #[must_use]
    pub fn function_def(&self, params: &[&str], body: Block) -> Expression {
        self.function_def_full(
            Vec::new(),
            FunctionSignature { params: self.params(params), ..FunctionSignature::default() },
            body,
        )
    }

    /// Full-fidelity anonymous function. Build `attributes` with
    /// [`Synth::function_attribute`].
    #[must_use]
    pub fn function_def_full(
        &self,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Expression {
        Expression::FunctionDef(Box::new(FunctionDef {
            span: self.next_span(),
            attributes,
            body: self.function_body(signature, body),
        }))
    }

    /// `function a.b.c(params) body end` / `function a.b:m(params) body end`.
    /// For attributes, generics, typed parameters, varargs, or a return
    /// type, use [`Synth::function_decl_full`].
    #[must_use]
    pub fn function_decl(
        &self,
        name_path: &[&str],
        method: Option<&str>,
        params: &[&str],
        body: Block,
    ) -> Statement {
        self.function_decl_full(
            name_path,
            method,
            Vec::new(),
            FunctionSignature { params: self.params(params), ..FunctionSignature::default() },
            body,
        )
    }

    /// Full-fidelity function declaration. Build `attributes` with
    /// [`Synth::function_attribute`].
    #[must_use]
    pub fn function_decl_full(
        &self,
        name_path: &[&str],
        method: Option<&str>,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::FunctionDecl(Box::new(FunctionDecl {
            span: self.next_span(),
            attributes,
            name: self.function_name(name_path, method),
            body: self.function_body(signature, body),
        }))
    }

    /// `local function name(params) body end`. For attributes or a full
    /// signature, use [`Synth::local_function_full`].
    #[must_use]
    pub fn local_function(&self, name: &str, params: &[&str], body: Block) -> Statement {
        self.local_function_full(
            name,
            Vec::new(),
            FunctionSignature { params: self.params(params), ..FunctionSignature::default() },
            body,
        )
    }

    /// Full-fidelity local function. Build `attributes` with
    /// [`Synth::function_attribute`].
    #[must_use]
    pub fn local_function_full(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::LocalFunction(Box::new(
            self.local_function_node(name, attributes, signature, body),
        ))
    }

    /// Luau `const function name(params) body end`, a read-only local
    /// function binding.
    #[must_use]
    pub fn const_function(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::LocalFunction(Box::new(LocalFunction {
            is_const: true,
            ..self.local_function_node(name, attributes, signature, body)
        }))
    }

    /// Luau `export function name(...) ... end`.
    #[must_use]
    pub fn export_function(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::LocalFunction(Box::new(LocalFunction {
            is_const: true,
            is_exported: true,
            ..self.local_function_node(name, attributes, signature, body)
        }))
    }

    /// A plain `local function`, for the callers to flag.
    fn local_function_node(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> LocalFunction {
        LocalFunction {
            span: self.next_span(),
            attributes,
            name: self.identifier(name),
            body: self.function_body(signature, body),
            is_const: false,
            is_exported: false,
        }
    }

    /// Lua 5.5 `global function name(params) body end`.
    #[must_use]
    pub fn global_function(
        &self,
        name: &str,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::GlobalFunction(Box::new(GlobalFunction {
            span: self.next_span(),
            name: self.identifier(name),
            body: self.function_body(signature, body),
        }))
    }

    fn function_name(&self, name_path: &[&str], method: Option<&str>) -> FunctionName {
        let names: Vec<Token> = name_path.iter().map(|part| self.identifier(part)).collect();
        FunctionName {
            span: self.next_span(),
            names,
            method: method.map(|name| self.identifier(name)),
        }
    }

    /// A Luau function attribute: `@name` when `args` is `None`, the
    /// bracketed `@[name(args)]` form otherwise.
    #[must_use]
    pub fn function_attribute(
        &self,
        name: &str,
        args: Option<Vec<Expression>>,
    ) -> FunctionAttribute {
        FunctionAttribute {
            span: self.next_span(),
            name: self.identifier(name),
            args: args.map(Punctuated::from_items),
        }
    }

    fn function_body(&self, signature: FunctionSignature, block: Block) -> FunctionBody {
        FunctionBody {
            span: self.next_span(),
            generics: signature.generics.map(Box::new),
            params: Punctuated::from_items(signature.params),
            vararg: signature.vararg.map(Box::new),
            return_type: signature.return_type,
            block,
        }
    }

    /// A Luau backtick string from text and expressions in source order:
    /// `` `hi {name}!` `` is `vec![Text("hi "), Expr(name), Text("!")]`.
    ///
    /// Text is unescaped; this escapes `` ` ``, `{`, `\`, and control
    /// characters. Adjacent text parts merge, and the lexer's
    /// `InterpolatedStringBegin`/`InterpolatedStringMiddle`/`InterpolatedStringEnd`
    /// segments are built for you.
    #[must_use]
    pub fn interpolated_string(&self, parts: Vec<InterpolatedStringPart<'_>>) -> Expression {
        let span = self.next_span();
        let mut pending_segments: Vec<(CompactString, Option<Expression>)> = Vec::new();
        let mut text = String::new();
        for part in parts {
            match part {
                InterpolatedStringPart::Text(more) => text.push_str(more),
                InterpolatedStringPart::Expr(expr) => {
                    pending_segments.push((escape_interpolated_text(&text), Some(expr)));
                    text.clear();
                }
            }
        }
        if pending_segments.is_empty() {
            // The lexer gives plain text as `InterpolatedStringBegin("")` followed by
            // `InterpolatedStringEnd(text)`.
            pending_segments.push((CompactString::from(""), None));
        }
        let last_index = pending_segments.len();
        pending_segments.push((escape_interpolated_text(&text), None));
        let segments: Vec<InterpolatedStringSegment> = pending_segments
            .into_iter()
            .enumerate()
            .map(|(index, (segment_text, expr))| {
                let literal = if index == 0 {
                    self.token(TokenKind::InterpolatedStringBegin(segment_text))
                } else if index == last_index {
                    self.token(TokenKind::InterpolatedStringEnd(segment_text))
                } else {
                    self.token(TokenKind::InterpolatedStringMiddle(segment_text))
                };
                InterpolatedStringSegment { literal, expr }
            })
            .collect();
        Expression::InterpolatedString(Box::new(InterpolatedString { span, segments }))
    }

    /// Luau `if c then a {elseif c2 then b} else z` expression.
    #[must_use]
    pub fn if_expr(
        &self,
        condition: Expression,
        then_expr: Expression,
        elseifs: Vec<(Expression, Expression)>,
        else_expr: Expression,
    ) -> Expression {
        let span = self.next_span();
        let elseif_clauses: Vec<ElseIfExprClause> = elseifs
            .into_iter()
            .map(|(elseif_condition, expr)| ElseIfExprClause {
                span: self.next_span(),
                condition: elseif_condition,
                expr,
            })
            .collect();
        Expression::IfExpression(Box::new(IfExpression {
            span,
            condition,
            then_expr,
            elseif_clauses,
            else_expr,
        }))
    }

    /// `local names = exprs` with plain untyped names. For type annotations
    /// or Lua 5.4 attributes, use [`Synth::local_full`].
    #[must_use]
    pub fn local(&self, names: &[&str], exprs: Vec<Expression>) -> Statement {
        self.local_full(
            names.iter().map(|name| self.attributed_name(name, None, None)).collect(),
            exprs,
        )
    }

    #[must_use]
    pub fn local_full(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        Statement::LocalAssignment(Box::new(self.local_assignment(names, exprs)))
    }

    /// Luau `const names = exprs`, a `local` whose names are read-only.
    ///
    /// # Panics
    ///
    /// Panics if `exprs` is empty; the grammar requires an initializer.
    #[must_use]
    pub fn const_local(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        assert!(!exprs.is_empty(), "synth: const declarations require an initializer");
        Statement::LocalAssignment(Box::new(LocalAssignment {
            is_const: true,
            ..self.local_assignment(names, exprs)
        }))
    }

    /// Luau `export local names [= exprs]`.
    #[must_use]
    pub fn export_local(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        Statement::LocalAssignment(Box::new(LocalAssignment {
            is_exported: true,
            ..self.local_assignment(names, exprs)
        }))
    }

    /// Luau `export const names = exprs`.
    ///
    /// # Panics
    ///
    /// Panics if `exprs` is empty; the grammar requires an initializer.
    #[must_use]
    pub fn export_const(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        assert!(!exprs.is_empty(), "synth: exported const declarations require an initializer");
        Statement::LocalAssignment(Box::new(LocalAssignment {
            is_const: true,
            is_exported: true,
            ..self.local_assignment(names, exprs)
        }))
    }

    /// A plain `local`, for the callers to flag.
    fn local_assignment(
        &self,
        names: Vec<AttributedName>,
        exprs: Vec<Expression>,
    ) -> LocalAssignment {
        LocalAssignment {
            span: self.next_span(),
            names: Punctuated::from_items(names),
            exprs: if exprs.is_empty() { None } else { Some(Punctuated::from_items(exprs)) },
            is_const: false,
            is_exported: false,
        }
    }

    /// One declared name for [`Synth::local_full`] / [`Synth::global_declaration`],
    /// with an optional Luau type annotation or Lua 5.4+ attribute (`"const"`
    /// or `"close"`). No dialect has both.
    #[must_use]
    pub fn attributed_name(
        &self,
        name: &str,
        ty: Option<Type>,
        attribute: Option<&str>,
    ) -> AttributedName {
        AttributedName {
            name: self.identifier(name),
            type_annotation: ty,
            attribute: attribute.map(|attribute_name| {
                Box::new(Attribute {
                    span: self.next_span(),
                    name: self.identifier(attribute_name),
                })
            }),
        }
    }

    /// Lua 5.5 `global names [= exprs]` declaration.
    #[must_use]
    pub fn global_declaration(
        &self,
        names: Vec<AttributedName>,
        exprs: Vec<Expression>,
    ) -> Statement {
        Statement::GlobalDeclaration(Box::new(GlobalDeclaration {
            span: self.next_span(),
            names: Punctuated::from_items(names),
            exprs: if exprs.is_empty() { None } else { Some(Punctuated::from_items(exprs)) },
        }))
    }

    /// Lua 5.5 `global *` / `global <attrib> *`.
    #[must_use]
    pub fn global_star(&self, attribute: Option<&str>) -> Statement {
        Statement::GlobalStar(Box::new(GlobalStar {
            span: self.next_span(),
            attribute: attribute
                .map(|name| Attribute { span: self.next_span(), name: self.identifier(name) }),
        }))
    }

    /// `targets = values`. Build targets with [`Synth::name_var`],
    /// [`Synth::index_var`], [`Synth::field_var`].
    #[must_use]
    pub fn assign(&self, targets: Vec<Var>, values: Vec<Expression>) -> Statement {
        Statement::Assignment(Box::new(Assignment {
            span: self.next_span(),
            targets: Punctuated::from_items(targets),
            values: Punctuated::from_items(values),
        }))
    }

    /// Luau compound assignment: `target op= value`.
    #[must_use]
    pub fn compound_assign(&self, target: Var, op: CompoundOp, value: Expression) -> Statement {
        Statement::CompoundAssignment(Box::new(CompoundAssignment {
            span: self.next_span(),
            var: target,
            op,
            expr: value,
        }))
    }

    /// A call expression as a statement.
    ///
    /// # Panics
    ///
    /// Panics if `call_expr` isn't a function call.
    #[must_use]
    #[expect(clippy::panic, reason = "documented panic")]
    pub fn call_stmt(&self, call_expr: Expression) -> Statement {
        let Expression::FunctionCall(mut call) = call_expr else {
            panic!("synth: call_stmt requires a function-call expression, got {call_expr:?}");
        };
        // The statement takes the call's span, so give it a fresh one: two
        // statements wrapping clones of one call need distinct comment anchors.
        call.span = self.next_span();
        Statement::FunctionCall(call)
    }

    #[must_use]
    pub fn do_block(&self, block: Block) -> Statement {
        Statement::DoBlock(Box::new(DoBlock { span: self.next_span(), block }))
    }

    #[must_use]
    pub fn while_(&self, condition: Expression, block: Block) -> Statement {
        Statement::WhileLoop(Box::new(WhileLoop { span: self.next_span(), condition, block }))
    }

    #[must_use]
    pub fn repeat_(&self, block: Block, condition: Expression) -> Statement {
        Statement::RepeatLoop(Box::new(RepeatLoop { span: self.next_span(), block, condition }))
    }

    #[must_use]
    pub fn if_(
        &self,
        condition: Expression,
        then_block: Block,
        elseifs: Vec<(Expression, Block)>,
        else_block: Option<Block>,
    ) -> Statement {
        let span = self.next_span();
        let elseif_clauses: Vec<ElseIfClause> = elseifs
            .into_iter()
            .map(|(elseif_condition, block)| ElseIfClause {
                span: self.next_span(),
                condition: elseif_condition,
                block,
            })
            .collect();
        let else_clause = else_block.map(|block| ElseClause { span: self.next_span(), block });
        Statement::IfStatement(Box::new(IfStatement {
            span,
            condition,
            block: then_block,
            elseif_clauses,
            else_clause,
        }))
    }

    /// `for variable = start, limit [, step] do block end`. Build `variable`
    /// with [`Synth::param`] or [`Synth::param_typed`].
    #[must_use]
    pub fn numeric_for(
        &self,
        variable: Parameter,
        start: Expression,
        limit: Expression,
        step: Option<Expression>,
        block: Block,
    ) -> Statement {
        Statement::NumericFor(Box::new(NumericFor {
            span: self.next_span(),
            name: variable.name,
            type_annotation: variable.type_annotation,
            start,
            limit,
            step,
            block,
        }))
    }

    /// `for names in exprs do block end`. Build names with [`Synth::param`]
    /// or [`Synth::param_typed`].
    #[must_use]
    pub fn generic_for(
        &self,
        names: Vec<Parameter>,
        exprs: Vec<Expression>,
        block: Block,
    ) -> Statement {
        Statement::GenericFor(Box::new(GenericFor {
            span: self.next_span(),
            names: Punctuated::from_items(names),
            exprs: Punctuated::from_items(exprs),
            block,
        }))
    }

    /// `goto label` (Lua 5.2+).
    #[must_use]
    pub fn goto_(&self, label: &str) -> Statement {
        Statement::Goto(Box::new(GotoStatement {
            span: self.next_span(),
            name: self.identifier(label),
        }))
    }

    /// `::name::` label (Lua 5.2+).
    #[must_use]
    pub fn label(&self, name: &str) -> Statement {
        Statement::Label(Box::new(LabelStatement {
            span: self.next_span(),
            name: self.identifier(name),
        }))
    }

    /// A mid-block `break` (Lua 5.2+). Lua 5.1 and Luau only allow `break` as
    /// the last statement, so use [`Synth::break_`] there.
    #[must_use]
    pub fn break_stmt(&self) -> Statement {
        Statement::Break(self.next_span())
    }

    /// Bare `;` (Lua 5.2+). The formatter drops these; only compact output
    /// prints them.
    #[must_use]
    pub fn empty_stmt(&self) -> Statement {
        Statement::EmptyStatement(self.next_span())
    }

    #[must_use]
    pub fn return_(&self, exprs: Vec<Expression>) -> LastStatement {
        LastStatement::Return(Box::new(ReturnStatement {
            span: self.next_span(),
            exprs: Punctuated::from_items(exprs),
        }))
    }

    #[must_use]
    pub fn break_(&self) -> LastStatement {
        LastStatement::Break(self.next_span())
    }

    /// Luau `continue`.
    #[must_use]
    pub fn continue_(&self) -> LastStatement {
        LastStatement::Continue(self.next_span())
    }

    #[must_use]
    pub fn block(&self, stmts: Vec<Statement>, last: Option<LastStatement>) -> Block {
        Block { span: self.next_span(), stmts, last_stmt: last.map(Box::new) }
    }

    #[must_use]
    pub fn param(&self, name: &str) -> Parameter {
        Parameter { span: self.next_span(), name: self.identifier(name), type_annotation: None }
    }

    #[must_use]
    pub fn param_typed(&self, name: &str, ty: Type) -> Parameter {
        Parameter { span: self.next_span(), name: self.identifier(name), type_annotation: Some(ty) }
    }

    /// A trailing `...` parameter, optionally with a Luau type annotation.
    /// Lua 5.5's named `...name` form isn't supported, so `name` is `None`.
    #[must_use]
    pub fn vararg_param(&self, type_annotation: Option<Type>) -> VarArgParam {
        VarArgParam { span: self.next_span(), name: None, type_annotation }
    }

    fn params(&self, names: &[&str]) -> Vec<Parameter> {
        names.iter().map(|name| self.param(name)).collect()
    }

    #[must_use]
    pub fn ty_named(&self, name: &str) -> Type {
        Type::Name { name: self.identifier_name(name), span: self.next_span() }
    }

    #[must_use]
    pub fn ty_qualified(&self, module: &str, name: &str) -> Type {
        Type::Named(Box::new(NamedType {
            span: self.next_span(),
            prefix: Some(self.identifier(module)),
            name: self.identifier(name),
            generics: None,
        }))
    }

    #[must_use]
    pub fn ty_generic(&self, name: &str, args: Vec<Type>) -> Type {
        Type::Named(Box::new(NamedType {
            span: self.next_span(),
            prefix: None,
            name: self.identifier(name),
            generics: Some(TypeArgs { span: self.next_span(), args: Punctuated::from_items(args) }),
        }))
    }

    /// `typeof(expr)`.
    #[must_use]
    pub fn ty_typeof(&self, expr: Expression) -> Type {
        Type::Typeof(Box::new(TypeofType { span: self.next_span(), expr }))
    }

    /// Parenthesizes a union, intersection, or function type so it stays a
    /// single member of the type being built: `(A | B)?`, `(() -> A) | B`.
    fn parenthesize_if_composite(&self, inner: Type) -> Type {
        match inner {
            Type::Union(_) | Type::Intersection(_) | Type::Function(_) => {
                self.ty_parenthesized(inner)
            }
            Type::Name { .. }
            | Type::Named(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Optional(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Singleton(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_) => inner,
        }
    }

    #[must_use]
    pub fn ty_optional(&self, inner: Type) -> Type {
        Type::Optional(Box::new(OptionalType {
            span: self.next_span(),
            type_value: self.parenthesize_if_composite(inner),
        }))
    }

    #[must_use]
    pub fn ty_union(&self, types: Vec<Type>) -> Type {
        Type::Union(Box::new(UnionType {
            span: self.next_span(),
            has_leading_pipe: false,
            types: Punctuated::from_items(
                types.into_iter().map(|inner| self.parenthesize_if_composite(inner)).collect(),
            ),
        }))
    }

    #[must_use]
    pub fn ty_intersection(&self, types: Vec<Type>) -> Type {
        Type::Intersection(Box::new(IntersectionType {
            span: self.next_span(),
            has_leading_ampersand: false,
            types: Punctuated::from_items(
                types
                    .into_iter()
                    // Luau rejects a `?` suffix beside `&`, so an optional
                    // member needs parens here but not in a union.
                    .map(|inner| {
                        if matches!(inner, Type::Optional(_)) {
                            self.ty_parenthesized(inner)
                        } else {
                            self.parenthesize_if_composite(inner)
                        }
                    })
                    .collect(),
            ),
        }))
    }

    /// `(T)`.
    #[must_use]
    pub fn ty_parenthesized(&self, inner: Type) -> Type {
        Type::Parenthesized(Box::new(ParenthesizedType {
            span: self.next_span(),
            type_value: inner,
        }))
    }

    /// `{ T }` array shorthand.
    #[must_use]
    pub fn ty_table_array(&self, element: Type) -> Type {
        self.ty_table(vec![TypeFieldSpec::Array(element)])
    }

    /// `{ name: T, ... }` named-field table type.
    #[must_use]
    pub fn ty_record(&self, fields: Vec<(&str, Type)>) -> Type {
        self.ty_table(
            fields
                .into_iter()
                .map(|(name, value)| TypeFieldSpec::Named { access: None, name, value })
                .collect(),
        )
    }

    /// A full table type mixing named fields, indexers, and an array element.
    /// Named fields and indexers can take a `read`/`write` access modifier.
    #[must_use]
    pub fn ty_table(&self, fields: Vec<TypeFieldSpec<'_>>) -> Type {
        let span = self.next_span();
        let built_fields = fields
            .into_iter()
            .map(|field| match field {
                TypeFieldSpec::Named { access, name, value } => TypeField::Named {
                    span: self.next_span(),
                    access: access.map(|modifier| self.access_token(modifier)),
                    name: self.identifier(name),
                    value,
                },
                TypeFieldSpec::Indexer { access, key, value } => TypeField::Indexer {
                    span: self.next_span(),
                    access: access.map(|modifier| self.access_token(modifier)),
                    key,
                    value,
                },
                TypeFieldSpec::Array(value) => TypeField::Array { span: self.next_span(), value },
            })
            .collect();
        Type::Table(Box::new(TableType { span, fields: Punctuated::from_items(built_fields) }))
    }

    fn access_token(&self, access: TypeFieldAccess) -> Token {
        self.identifier(match access {
            TypeFieldAccess::Read => "read",
            TypeFieldAccess::Write => "write",
        })
    }

    /// `(T, U) -> R` with unnamed parameters. For generics or named
    /// parameters, use [`Synth::ty_function_full`].
    #[must_use]
    pub fn ty_function(&self, params: Vec<Type>, return_type: Type) -> Type {
        self.ty_function_full(None, params.into_iter().map(|ty| (None, ty)).collect(), return_type)
    }

    /// `<T...>(name: T, U) -> R`. Each parameter is `(optional name, type)`.
    /// For a pack return, pass a [`Synth::ty_pack`].
    #[must_use]
    pub fn ty_function_full(
        &self,
        generics: Option<GenericTypeList>,
        params: Vec<(Option<&str>, Type)>,
        return_type: Type,
    ) -> Type {
        let span = self.next_span();
        let typed_params: Vec<FunctionTypeParam> = params
            .into_iter()
            .map(|(param_name, type_value)| FunctionTypeParam {
                span: self.next_span(),
                name: param_name.map(|name| self.identifier(name)),
                type_value,
            })
            .collect();
        Type::Function(Box::new(FunctionType {
            span,
            generics,
            params: Punctuated::from_items(typed_params),
            return_type,
        }))
    }

    #[must_use]
    pub fn ty_singleton_string(&self, content: &str) -> Type {
        Type::Singleton(self.token(TokenKind::StringLiteral(quote_utf8(content))))
    }

    #[must_use]
    pub fn ty_singleton_bool(&self, value: bool) -> Type {
        let kind = if value { TokenKind::True } else { TokenKind::False };
        Type::Singleton(self.token(kind))
    }

    #[must_use]
    pub fn ty_singleton_nil(&self) -> Type {
        Type::Singleton(self.token(TokenKind::Nil))
    }

    #[must_use]
    pub fn ty_pack(&self, types: Vec<Type>) -> Type {
        let span = self.next_span();
        Type::Pack(Box::new(TypePack { span, types: Punctuated::from_items(types) }))
    }

    /// `...T` variadic pack element.
    #[must_use]
    pub fn ty_variadic(&self, element: Type) -> Type {
        Type::Variadic(Box::new(VariadicType { span: self.next_span(), type_value: element }))
    }

    /// `T...` generic pack reference.
    #[must_use]
    pub fn ty_generic_pack(&self, name: &str) -> Type {
        Type::GenericPack(Box::new(GenericPackType {
            span: self.next_span(),
            name: self.identifier(name),
        }))
    }

    /// A generic parameter list at a declaration site: `<T, U...>`. Each pair
    /// is a name and whether it's a pack (`T...`). Defaults aren't supported.
    #[must_use]
    pub fn generic_type_list(&self, params: Vec<(&str, bool)>) -> GenericTypeList {
        let span = self.next_span();
        let generic_params: Vec<GenericTypeParam> = params
            .into_iter()
            .map(|(name, is_pack)| GenericTypeParam {
                span: self.next_span(),
                name: self.identifier(name),
                is_pack,
                default: None,
            })
            .collect();
        GenericTypeList { span, params: Punctuated::from_items(generic_params) }
    }

    /// Luau alias `[export] type Name [<generics>] = T`.
    #[must_use]
    pub fn type_declaration(
        &self,
        is_exported: bool,
        name: &str,
        generics: Option<GenericTypeList>,
        value: Type,
    ) -> Statement {
        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span: self.next_span(),
            is_exported,
            name: self.identifier(name),
            generics: generics.map(Box::new),
            type_value: TypeDeclarationValue::Alias(value),
        }))
    }

    /// Luau `[export] type function Name(params) body end`, a compile-time
    /// type function whose body is ordinary Luau.
    #[must_use]
    pub fn type_function(
        &self,
        is_exported: bool,
        name: &str,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span: self.next_span(),
            is_exported,
            name: self.identifier(name),
            generics: None,
            type_value: TypeDeclarationValue::TypeFunction(Box::new(
                self.function_body(signature, body),
            )),
        }))
    }

    fn parenthesized_args(&self, args: Vec<Expression>) -> FunctionArgs {
        FunctionArgs::Parenthesized { span: self.next_span(), args: Punctuated::from_items(args) }
    }
}

/// Double-quoted token text for UTF-8 `content`. Multibyte characters pass
/// through; quotes, backslashes, and control characters are escaped.
fn quote_utf8(content: &str) -> CompactString {
    let mut raw = String::with_capacity(content.len().saturating_add(2));
    raw.push('"');
    for character in content.chars() {
        match character {
            '\\' => raw.push_str("\\\\"),
            '"' => raw.push_str("\\\""),
            '\n' => raw.push_str("\\n"),
            '\r' => raw.push_str("\\r"),
            '\t' => raw.push_str("\\t"),
            '\0'..='\x1f' | '\x7f' => raw.push_str(&decimal_escape(u32::from(character))),
            _ => raw.push(character),
        }
    }
    raw.push('"');
    CompactString::from(raw)
}

/// Double-quoted token text for arbitrary bytes. Everything outside
/// printable ASCII is escaped, so the result is valid UTF-8 for any input.
fn quote_bytes(content: &[u8]) -> CompactString {
    let mut raw = String::with_capacity(content.len().saturating_add(2));
    raw.push('"');
    for &byte in content {
        match byte {
            b'\\' => raw.push_str("\\\\"),
            b'"' => raw.push_str("\\\""),
            b'\n' => raw.push_str("\\n"),
            b'\r' => raw.push_str("\\r"),
            b'\t' => raw.push_str("\\t"),
            0x20..=0x7e => raw.push(char::from(byte)),
            other => raw.push_str(&decimal_escape(u32::from(other))),
        }
    }
    raw.push('"');
    CompactString::from(raw)
}

/// Escapes interpolated-string segment text. The lexer stores segment text
/// as written, escapes included, so synthesized text needs them too.
fn escape_interpolated_text(text: &str) -> CompactString {
    let mut raw = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => raw.push_str("\\\\"),
            '`' => raw.push_str("\\`"),
            '{' => raw.push_str("\\{"),
            '\n' => raw.push_str("\\n"),
            '\r' => raw.push_str("\\r"),
            '\t' => raw.push_str("\\t"),
            '\0'..='\x1f' | '\x7f' => raw.push_str(&decimal_escape(u32::from(character))),
            _ => raw.push(character),
        }
    }
    CompactString::from(raw)
}

/// Always three digits: a shorter escape followed by a literal digit
/// (`\57`) would be read as a different byte.
fn decimal_escape(byte_value: u32) -> String {
    format!("\\{byte_value:03}")
}

#[cfg(test)]
mod tests {
    use core::iter::repeat_with;
    use std::thread;

    use rustc_hash::FxHashSet;

    use super::*;

    fn number_text(expr: Expression) -> CompactString {
        let Expression::Number(literal) = expr else {
            panic!("expected number");
        };
        literal.text
    }

    fn string_text(expr: Expression) -> CompactString {
        let Expression::StringLiteral(literal) = expr else {
            panic!("expected string literal");
        };
        literal.text
    }

    #[test]
    fn builds_local_with_binary_op() {
        let synth = Synth::new();
        let sum = synth.binary_op(synth.number("1"), BinOp::Add, synth.number("2"));
        let stmt = synth.local(&["x"], vec![sum]);

        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        assert_eq!(local.names.len(), 1);
        let exprs = local.exprs.as_ref().unwrap();
        assert!(matches!(exprs.first().unwrap(), Expression::BinaryOp(_)));
    }

    #[test]
    fn binary_op_parenthesizes_lower_precedence_operands() {
        let synth = Synth::new();
        // (a + b) * c: the additive LHS must gain parens under `*`.
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let product = synth.binary_op(sum, BinOp::Mul, synth.name_expr("c"));

        let Expression::BinaryOp(binary_op) = &product else {
            panic!("expected binop");
        };
        assert!(
            matches!(binary_op.left, Expression::Parenthesized(_)),
            "lower-precedence LHS not parenthesized"
        );
    }

    #[test]
    fn binary_op_keeps_higher_precedence_operands_bare() {
        let synth = Synth::new();
        // a * b + c: the multiplicative LHS needs no parens under `+`.
        let product = synth.binary_op(synth.name_expr("a"), BinOp::Mul, synth.name_expr("b"));
        let sum = synth.binary_op(product, BinOp::Add, synth.name_expr("c"));

        let Expression::BinaryOp(binary_op) = &sum else {
            panic!("expected binop");
        };
        assert!(matches!(binary_op.left, Expression::BinaryOp(_)));
    }

    #[test]
    fn binary_op_honors_associativity() {
        let synth = Synth::new();
        // Left-nested concat needs parens: (a .. b) .. c is not a .. b .. c.
        let inner_concat =
            synth.binary_op(synth.name_expr("a"), BinOp::Concat, synth.name_expr("b"));
        let concat = synth.binary_op(inner_concat, BinOp::Concat, synth.name_expr("c"));
        let Expression::BinaryOp(concat_node) = &concat else {
            panic!("expected binop");
        };
        assert!(matches!(concat_node.left, Expression::Parenthesized(_)));

        // Right-nested subtraction needs parens: a - (b - c).
        let inner_difference =
            synth.binary_op(synth.name_expr("b"), BinOp::Sub, synth.name_expr("c"));
        let difference = synth.binary_op(synth.name_expr("a"), BinOp::Sub, inner_difference);
        let Expression::BinaryOp(difference_node) = &difference else {
            panic!("expected binop");
        };
        assert!(matches!(difference_node.right, Expression::Parenthesized(_)));
    }

    #[test]
    fn caret_parenthesizes_unary_lhs() {
        let synth = Synth::new();
        // (-a)^b: without parens this would re-parse as -(a^b).
        let negated_base = synth.unary_op(UnOp::Neg, synth.name_expr("a"));
        let power = synth.binary_op(negated_base, BinOp::Pow, synth.name_expr("b"));
        let Expression::BinaryOp(power_node) = &power else {
            panic!("expected binop");
        };
        assert!(matches!(power_node.left, Expression::Parenthesized(_)));

        // a * -b stays bare: unary is always accepted after a binary_op.
        let negated_factor = synth.unary_op(UnOp::Neg, synth.name_expr("b"));
        let product = synth.binary_op(synth.name_expr("a"), BinOp::Mul, negated_factor);
        let Expression::BinaryOp(product_node) = &product else {
            panic!("expected binop");
        };
        assert!(matches!(product_node.right, Expression::UnaryOp(_)));
    }

    #[test]
    fn if_expression_operands_always_parenthesized() {
        let synth = Synth::new();
        let if_expr =
            synth.if_expr(synth.boolean(true), synth.number("1"), vec![], synth.number("2"));
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, if_expr);
        let Expression::BinaryOp(binary_op) = &sum else {
            panic!("expected binop");
        };
        assert!(
            matches!(binary_op.right, Expression::Parenthesized(_)),
            "greedy if-expression must be parenthesized as an operand"
        );
    }

    #[test]
    fn unary_op_parenthesizes_binary_operand() {
        let synth = Synth::new();
        // -(a + b): without parens this would re-parse as (-a) + b.
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let negated = synth.unary_op(UnOp::Neg, sum);
        let Expression::UnaryOp(unary_op) = &negated else {
            panic!("expected unop");
        };
        assert!(matches!(unary_op.operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn type_cast_parenthesizes_compound_operand() {
        let synth = Synth::new();
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let cast = synth.type_cast(sum, synth.ty_named("number"));
        let Expression::TypeCast(type_cast) = &cast else {
            panic!("expected type cast");
        };
        assert!(matches!(type_cast.expr, Expression::Parenthesized(_)));
    }

    #[test]
    fn prefix_positions_wrap_non_prefix_expressions() {
        let synth = Synth::new();
        // A string receiver must be parenthesized: ("s"):rep(2).
        let called = synth.method_call(synth.string("s"), "rep", vec![synth.number("2")]);
        let Expression::FunctionCall(call) = &called else {
            panic!("expected call");
        };
        assert!(matches!(call.callee, Expression::Parenthesized(_)));

        // A table prefix must be parenthesized: ({}).x.
        let accessed = synth.field(synth.table(vec![]), "x");
        let Expression::Var(Var::FieldAccess(table_access)) = &accessed else {
            panic!("expected field access");
        };
        assert!(matches!(table_access.prefix, Expression::Parenthesized(_)));

        // A call prefix stays bare: f().x.
        let chained = synth.field(synth.call(synth.name_expr("f"), vec![]), "x");
        let Expression::Var(Var::FieldAccess(call_access)) = &chained else {
            panic!("expected field access");
        };
        assert!(matches!(call_access.prefix, Expression::FunctionCall(_)));
    }

    #[test]
    fn field_or_index_picks_form_by_name() {
        let synth = Synth::new();
        assert!(matches!(
            synth.field_or_index(synth.name_expr("t"), "valid_name"),
            Expression::Var(Var::FieldAccess(_))
        ));
        for bad in ["not an ident", "1st", "", "end", "goto"] {
            assert!(
                matches!(
                    synth.field_or_index(synth.name_expr("t"), bad),
                    Expression::Var(Var::Index(_))
                ),
                "{bad:?} should use the bracketed form"
            );
        }
    }

    #[test]
    fn number_f64_handles_special_values() {
        let synth = Synth::new();

        assert_eq!(number_text(synth.number_f64(1.5)), "1.5");

        // Integral floats keep the float subtype on 5.3+.
        assert_eq!(number_text(synth.number_f64(3.0)), "3.0");

        // Negatives become a unary-minus node (Lua has no negative literals).
        assert!(matches!(synth.number_f64(-2.5), Expression::UnaryOp(_)));
        assert!(matches!(synth.number_f64(-0.0), Expression::UnaryOp(_)));

        // inf -> 1/0, nan -> 0/0, -inf -> -(1/0).
        assert!(matches!(synth.number_f64(f64::INFINITY), Expression::BinaryOp(_)));
        assert!(matches!(synth.number_f64(f64::NAN), Expression::BinaryOp(_)));
        let Expression::UnaryOp(negative_infinity) = synth.number_f64(f64::NEG_INFINITY) else {
            panic!("expected unop");
        };
        assert!(matches!(negative_infinity.operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn number_i64_handles_extremes() {
        let synth = Synth::new();
        assert_eq!(number_text(synth.number_i64(42)), "42");

        assert!(matches!(synth.number_i64(-7), Expression::UnaryOp(_)));

        // i64::MIN has no decimal literal; the hex form wraps to it.
        assert_eq!(number_text(synth.number_i64(i64::MIN)), "0x8000000000000000");
    }

    #[test]
    fn string_escaping_is_raw() {
        let synth = Synth::new();
        assert_eq!(string_text(synth.string("a\"b\n")), "\"a\\\"b\\n\"");
    }

    #[test]
    fn string_bytes_escapes_non_utf8() {
        let synth = Synth::new();
        // Escapes are zero-padded to three digits so the following literal
        // digit `7` can't extend them.
        assert_eq!(string_text(synth.string_bytes(&[0xff, b'a', 0x01, b'7'])), "\"\\255a\\0017\"");
    }

    #[test]
    fn long_string_picks_collision_free_level() {
        let synth = Synth::new();
        assert_eq!(string_text(synth.long_string("plain text")), "[[plain text]]");

        assert_eq!(string_text(synth.long_string("has ]] inside")), "[=[has ]] inside]=]");

        // A leading newline would be swallowed; it gets doubled.
        assert_eq!(string_text(synth.long_string("\ntext")), "[[\n\ntext]]");

        // A trailing `]` at level 0 would merge with the closer.
        assert_eq!(string_text(synth.long_string("x]")), "[=[x]]=]");
    }

    #[test]
    fn builds_if_with_elseif() {
        let synth = Synth::new();
        let stmt = synth.if_(
            synth.boolean(true),
            synth.block(vec![], None),
            vec![(synth.boolean(false), synth.block(vec![], None))],
            None,
        );

        let Statement::IfStatement(if_stmt) = &stmt else {
            panic!("expected if statement");
        };
        assert_eq!(if_stmt.elseif_clauses.len(), 1);
        assert!(if_stmt.else_clause.is_none());
    }

    #[test]
    fn builds_if_expr_with_elseif() {
        let synth = Synth::new();
        let expr = synth.if_expr(
            synth.boolean(true),
            synth.number("1"),
            vec![(synth.boolean(false), synth.number("2"))],
            synth.number("3"),
        );
        let Expression::IfExpression(if_expr) = &expr else {
            panic!("expected if expression");
        };
        assert_eq!(if_expr.elseif_clauses.len(), 1);
    }

    #[test]
    fn builds_typed_local() {
        let synth = Synth::new();
        let optional = synth.ty_optional(synth.ty_named("number"));
        let stmt = synth
            .local_full(vec![synth.attributed_name("x", Some(optional), None)], vec![synth.nil()]);

        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        let first = local.names.first().unwrap();
        let ty = first.type_annotation.as_ref().unwrap();
        assert!(matches!(ty, Type::Optional(_)));
    }

    #[test]
    fn builds_typed_function() {
        let synth = Synth::new();
        let signature = FunctionSignature {
            params: vec![synth.param_typed("n", synth.ty_named("number"))],
            return_type: Some(synth.ty_named("number")),
            ..FunctionSignature::default()
        };
        let return_stmt = synth.return_(vec![synth.name_expr("n")]);
        let function =
            synth.function_def_full(Vec::new(), signature, synth.block(vec![], Some(return_stmt)));

        let Expression::FunctionDef(function_def) = &function else {
            panic!("expected function def");
        };
        assert_eq!(function_def.body.params.len(), 1);
        assert!(function_def.body.params.first().unwrap().type_annotation.is_some());
        assert!(function_def.body.return_type.is_some());
    }

    #[test]
    fn builds_generic_function_with_attributes() {
        let synth = Synth::new();
        let signature = FunctionSignature {
            generics: Some(synth.generic_type_list(vec![("T", false)])),
            params: vec![synth.param_typed("value", synth.ty_named("T"))],
            return_type: Some(synth.ty_named("T")),
            ..FunctionSignature::default()
        };
        let stmt = synth.local_function_full(
            "id",
            vec![synth.function_attribute("native", None)],
            signature,
            synth.block(vec![], None),
        );

        let Statement::LocalFunction(local_function) = &stmt else {
            panic!("expected local function");
        };
        assert_eq!(local_function.attributes.len(), 1);
        assert!(local_function.body.generics.is_some());
    }

    #[test]
    fn builds_generic_for() {
        let synth = Synth::new();
        let call = synth.call(synth.name_expr("pairs"), vec![synth.name_expr("t")]);
        let stmt = synth.generic_for(
            vec![synth.param("k"), synth.param("v")],
            vec![call],
            synth.block(vec![], None),
        );

        let Statement::GenericFor(generic_for) = &stmt else {
            panic!("expected generic for");
        };
        assert_eq!(generic_for.names.len(), 2);
        assert_eq!(generic_for.exprs.len(), 1);
    }

    #[test]
    fn builds_typed_numeric_for() {
        let synth = Synth::new();
        let stmt = synth.numeric_for(
            synth.param_typed("i", synth.ty_named("number")),
            synth.number("1"),
            synth.number("10"),
            None,
            synth.block(vec![], None),
        );
        let Statement::NumericFor(numeric_for) = &stmt else {
            panic!("expected numeric for");
        };
        assert!(numeric_for.type_annotation.is_some());
    }

    #[test]
    fn builds_mixed_table() {
        let synth = Synth::new();
        let table = synth.table(vec![
            FieldSpec::Positional(synth.number("1")),
            FieldSpec::Named("a", synth.number("2")),
            FieldSpec::Bracketed(synth.string("k"), synth.boolean(true)),
        ]);

        let Expression::TableConstructor(constructor) = &table else {
            panic!("expected table constructor");
        };
        assert_eq!(constructor.fields.len(), 3);
        assert!(matches!(constructor.fields.items[0], Field::Positional { .. }));
        assert!(matches!(constructor.fields.items[1], Field::Named { .. }));
        assert!(matches!(constructor.fields.items[2], Field::Bracketed { .. }));
    }

    #[test]
    fn array_and_record_sugar() {
        let synth = Synth::new();
        let Expression::TableConstructor(array) =
            synth.array(vec![synth.number("1"), synth.number("2")])
        else {
            panic!("expected table");
        };
        assert!(array.fields.iter().all(|field| matches!(field, Field::Positional { .. })));

        // Unsafe keys fall back to the bracketed form.
        let Expression::TableConstructor(record) =
            synth.record(vec![("good", synth.number("1")), ("not valid", synth.number("2"))])
        else {
            panic!("expected table");
        };
        assert!(matches!(record.fields.items[0], Field::Named { .. }));
        assert!(matches!(record.fields.items[1], Field::Bracketed { .. }));
    }

    #[test]
    fn builds_sugar_call_forms() {
        let synth = Synth::new();
        let Expression::FunctionCall(string_call) =
            synth.call_string(synth.name_expr("require"), "module")
        else {
            panic!("expected call");
        };
        assert!(matches!(string_call.args, FunctionArgs::StringLiteral(_)));

        let Expression::FunctionCall(table_call) = synth.call_table(
            synth.name_expr("configure"),
            vec![FieldSpec::Named("debug", synth.boolean(true))],
        ) else {
            panic!("expected call");
        };
        assert!(matches!(table_call.args, FunctionArgs::TableConstructor(_)));

        let Expression::FunctionCall(method_call) =
            synth.method_call_string(synth.name_expr("s"), "gsub", "pattern")
        else {
            panic!("expected call");
        };
        assert!(method_call.method.is_some());
        assert!(matches!(method_call.args, FunctionArgs::StringLiteral(_)));
    }

    #[test]
    fn spans_are_monotonic() {
        let synth = Synth::new();
        let first = synth.nil();
        let second = synth.nil();
        let third = synth.number("3");
        assert!(second.span().start > first.span().start);
        assert!(third.span().start > second.span().start);
    }

    #[test]
    fn exhausted_spans_never_wrap_even_when_shared() {
        let synth = Synth::starting_at(u32::MAX - 1);
        let shared = synth.share();
        assert_eq!(synth.nil().span().start, u32::MAX - 1);
        for allocator in [&synth, &shared, &synth] {
            assert_eq!(allocator.nil().span().start, u32::MAX);
            assert_eq!(allocator.next_offset.load(Ordering::Relaxed), u32::MAX);
        }
    }

    #[test]
    fn exhaustion_under_contention_never_restarts_the_range() {
        // A check-then-increment guard would race here: one thread can wrap
        // the counter to zero while another is mid-allocation, handing out
        // offsets that collide with the range's first nodes.
        let start = u32::MAX - 4;
        let synth = Synth::starting_at(start);
        let mut offsets: Vec<u32> = Vec::new();
        thread::scope(|scope| {
            let handles: Vec<_> = repeat_with(|| {
                let shared = synth.share();
                scope.spawn(move || {
                    repeat_with(|| shared.nil().span().start).take(4).collect::<Vec<u32>>()
                })
            })
            .take(4)
            .collect();
            for handle in handles {
                offsets.extend(handle.join().unwrap());
            }
        });
        assert!(offsets.iter().all(|&offset| offset >= start), "{offsets:?}");
        // Only `u32::MAX` may repeat; every other offset is distinct however
        // the threads interleave.
        let allocated: Vec<u32> =
            offsets.iter().copied().filter(|&offset| offset < u32::MAX).collect();
        let unique: FxHashSet<u32> = allocated.iter().copied().collect();
        assert_eq!(unique.len(), allocated.len(), "{offsets:?}");
        assert_eq!(synth.next_offset.load(Ordering::Relaxed), u32::MAX);
    }

    #[test]
    fn starting_at_offsets_spans() {
        let synth = Synth::starting_at(1000);
        assert_eq!(synth.nil().span().start, 1000);
    }

    #[test]
    fn shared_synths_never_collide() {
        let synth = Synth::new();
        let shared = synth.share();
        let spans = [
            synth.nil().span().start,
            shared.nil().span().start,
            synth.nil().span().start,
            shared.nil().span().start,
        ];
        let unique: FxHashSet<u32> = spans.iter().copied().collect();
        assert_eq!(unique.len(), spans.len(), "{spans:?}");
    }

    #[test]
    fn call_stmt_spans_stay_unique_for_cloned_calls() {
        let synth = Synth::new();
        let call = synth.call(synth.name_expr("f"), vec![]);
        let first = synth.call_stmt(call.clone());
        let second = synth.call_stmt(call.clone());
        assert_ne!(first.span(), second.span());
        assert_ne!(first.span(), call.span());
    }

    #[test]
    fn shared_spans_unique_across_threads() {
        let synth = Synth::new();
        let mut all_spans: Vec<u32> = Vec::new();
        thread::scope(|scope| {
            let handles: Vec<_> = repeat_with(|| {
                let shared = synth.share();
                scope.spawn(move || {
                    repeat_with(|| shared.nil().span().start).take(250).collect::<Vec<u32>>()
                })
            })
            .take(4)
            .collect();
            for handle in handles {
                all_spans.extend(handle.join().unwrap());
            }
        });
        let unique: FxHashSet<u32> = all_spans.iter().copied().collect();
        assert_eq!(unique.len(), all_spans.len());
    }

    #[test]
    fn synth_is_send_and_sync() {
        fn assert_send_sync<T>()
        where
            T: Send + Sync,
        {
        }
        assert_send_sync::<Synth>();
    }

    #[test]
    fn share_preserves_version_pin() {
        let shared = Synth::new().with_version(LuaVersion::Luau).share();
        // `goto` is an identifier only under the pinned Luau dialect, so the
        // dot form proves the pin carried over.
        assert!(matches!(
            shared.field_or_index(shared.name_expr("t"), "goto"),
            Expression::Var(Var::FieldAccess(_))
        ));
    }

    #[test]
    fn builds_variadic_function() {
        let synth = Synth::new();
        let signature = FunctionSignature {
            params: vec![synth.param("first")],
            vararg: Some(synth.vararg_param(None)),
            ..FunctionSignature::default()
        };
        let function = synth.function_def_full(Vec::new(), signature, synth.block(vec![], None));

        let Expression::FunctionDef(function_def) = &function else {
            panic!("expected function def");
        };
        assert_eq!(function_def.body.params.len(), 1);
        let vararg = function_def.body.vararg.as_ref().expect("vararg present");
        assert!(vararg.name.is_none());
        assert!(vararg.type_annotation.is_none());
    }

    #[test]
    fn builds_typed_vararg_param() {
        let synth = Synth::new();
        let vararg = synth.vararg_param(Some(synth.ty_named("number")));
        assert!(vararg.name.is_none());
        assert!(vararg.type_annotation.is_some());
    }

    #[test]
    fn builds_continue_and_breaks() {
        let synth = Synth::new();
        assert!(matches!(synth.continue_(), LastStatement::Continue(_)));
        assert!(matches!(synth.break_(), LastStatement::Break(_)));
        assert!(matches!(synth.break_stmt(), Statement::Break(_)));
        assert!(matches!(synth.empty_stmt(), Statement::EmptyStatement(_)));
    }

    #[test]
    fn builds_globals() {
        let synth = Synth::new();
        let stmt = synth
            .global_declaration(vec![synth.attributed_name("shared", None, Some("const"))], vec![]);
        let Statement::GlobalDeclaration(declaration) = &stmt else {
            panic!("expected global declaration");
        };
        assert!(declaration.names.first().unwrap().attribute.is_some());

        let function =
            synth.global_function("main", FunctionSignature::default(), synth.block(vec![], None));
        assert!(matches!(function, Statement::GlobalFunction(_)));

        let Statement::GlobalStar(star) = synth.global_star(Some("const")) else {
            panic!("expected global star");
        };
        assert!(star.attribute.is_some());
    }

    #[test]
    fn builds_compound_assignment() {
        let synth = Synth::new();
        let stmt =
            synth.compound_assign(synth.name_var("counter"), CompoundOp::Add, synth.number("1"));

        let Statement::CompoundAssignment(compound) = &stmt else {
            panic!("expected compound assignment");
        };
        assert!(matches!(compound.var, Var::Name(_)));
        assert_eq!(compound.op, CompoundOp::Add);
    }

    #[test]
    fn builds_interpolated_string_with_escapes() {
        let synth = Synth::new();
        let inner = synth.name_expr("value");
        // InterpolatedStringBegin("a") carries the expression and InterpolatedStringEnd the rest.
        let string = synth.interpolated_string(vec![
            InterpolatedStringPart::Text("a"),
            InterpolatedStringPart::Expr(inner),
            InterpolatedStringPart::Text("b{`"),
        ]);

        let Expression::InterpolatedString(interpolated) = &string else {
            panic!("expected interpolated string");
        };
        assert_eq!(interpolated.segments.len(), 2);
        assert!(matches!(
            interpolated.segments[0].literal.kind,
            TokenKind::InterpolatedStringBegin(_)
        ));
        assert!(interpolated.segments[0].expr.is_some());
        let TokenKind::InterpolatedStringEnd(text) = &interpolated.segments[1].literal.kind else {
            panic!("expected InterpolatedStringEnd");
        };
        // `{` and backtick carry their escapes, as the lexer stores them.
        assert_eq!(text.as_str(), "b\\{\\`");
        assert!(interpolated.segments[1].expr.is_none());
    }

    #[test]
    fn interpolated_string_derives_segment_shape() {
        let synth = Synth::new();

        // Plain text keeps the lexer's Begin("") + End(text) pair; adjacent
        // text parts merge.
        let plain = synth.interpolated_string(vec![
            InterpolatedStringPart::Text("a"),
            InterpolatedStringPart::Text("b"),
        ]);
        let Expression::InterpolatedString(plain_node) = &plain else {
            panic!("expected interpolated string");
        };
        assert_eq!(plain_node.segments.len(), 2);
        let TokenKind::InterpolatedStringBegin(begin) = &plain_node.segments[0].literal.kind else {
            panic!("expected InterpolatedStringBegin");
        };
        assert_eq!(begin.as_str(), "");
        let TokenKind::InterpolatedStringEnd(end) = &plain_node.segments[1].literal.kind else {
            panic!("expected InterpolatedStringEnd");
        };
        assert_eq!(end.as_str(), "ab");

        // Two expressions produce a Mid segment between Begin and End.
        let multi = synth.interpolated_string(vec![
            InterpolatedStringPart::Expr(synth.name_expr("x")),
            InterpolatedStringPart::Text("-"),
            InterpolatedStringPart::Expr(synth.name_expr("y")),
        ]);
        let Expression::InterpolatedString(multi_node) = &multi else {
            panic!("expected interpolated string");
        };
        assert_eq!(multi_node.segments.len(), 3);
        assert!(matches!(
            multi_node.segments[1].literal.kind,
            TokenKind::InterpolatedStringMiddle(_)
        ));
        assert!(multi_node.segments[2].expr.is_none());
    }

    #[test]
    fn builds_goto_and_label() {
        let synth = Synth::new();
        assert!(matches!(synth.goto_("done"), Statement::Goto(_)));
        assert!(matches!(synth.label("done"), Statement::Label(_)));
    }

    #[test]
    fn builds_type_declaration_with_generics() {
        let synth = Synth::new();
        let type_params = synth.generic_type_list(vec![("T", false), ("Rest", true)]);
        let stmt = synth.type_declaration(true, "Alias", Some(type_params), synth.ty_named("T"));

        let Statement::TypeDeclaration(declaration) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(declaration.is_exported);
        assert!(matches!(declaration.type_value, TypeDeclarationValue::Alias(_)));
        let generics = declaration.generics.as_ref().expect("generics present");
        assert_eq!(generics.params.len(), 2);
        assert!(generics.params.last().unwrap().is_pack);
    }

    #[test]
    fn builds_type_function() {
        let synth = Synth::new();
        let signature =
            FunctionSignature { params: vec![synth.param("ty")], ..FunctionSignature::default() };
        let stmt = synth.type_function(false, "Partial", signature, synth.block(vec![], None));
        let Statement::TypeDeclaration(declaration) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(!declaration.is_exported);
        assert!(matches!(declaration.type_value, TypeDeclarationValue::TypeFunction(_)));
    }

    #[test]
    fn builds_full_table_type() {
        let synth = Synth::new();
        let ty = synth.ty_table(vec![
            TypeFieldSpec::Named {
                access: Some(TypeFieldAccess::Read),
                name: "id",
                value: synth.ty_named("number"),
            },
            TypeFieldSpec::Indexer {
                access: None,
                key: synth.ty_named("string"),
                value: synth.ty_named("boolean"),
            },
        ]);
        let Type::Table(table) = &ty else {
            panic!("expected table type");
        };
        assert!(matches!(&table.fields.items[0], TypeField::Named { access: Some(_), .. }));
        assert!(matches!(&table.fields.items[1], TypeField::Indexer { .. }));
    }

    #[test]
    fn builds_remaining_type_forms() {
        let synth = Synth::new();
        assert!(matches!(synth.ty_typeof(synth.name_expr("x")), Type::Typeof(_)));
        assert!(matches!(synth.ty_parenthesized(synth.ty_named("T")), Type::Parenthesized(_)));
        assert!(matches!(synth.ty_generic_pack("T"), Type::GenericPack(_)));
        assert!(matches!(
            synth.ty_singleton_bool(true),
            Type::Singleton(Token { kind: TokenKind::True, .. })
        ));
        assert!(matches!(
            synth.ty_singleton_nil(),
            Type::Singleton(Token { kind: TokenKind::Nil, .. })
        ));

        let named_param = synth.ty_function_full(
            Some(synth.generic_type_list(vec![("T", true)])),
            vec![(Some("x"), synth.ty_named("number"))],
            synth.ty_pack(vec![]),
        );
        let Type::Function(function) = &named_param else {
            panic!("expected function type");
        };
        assert!(function.generics.is_some());
        assert!(function.params.first().unwrap().name.is_some());
        assert!(matches!(function.return_type, Type::Pack(_)));
    }

    #[test]
    fn builds_function_declaration_with_method() {
        let synth = Synth::new();
        let stmt =
            synth.function_decl(&["a", "b"], Some("greet"), &["self"], synth.block(vec![], None));

        let Statement::FunctionDecl(function_decl) = &stmt else {
            panic!("expected function declaration");
        };
        assert_eq!(function_decl.name.names.len(), 2);
        assert!(function_decl.name.method.is_some());
    }

    #[test]
    fn comment_helpers_anchor_on_statements() {
        let synth = Synth::new();
        let stmt = synth.local(&["x"], vec![synth.number("1")]);
        let leading = SyntheticComment::leading(&stmt, "before");
        let trailing = SyntheticComment::trailing(&stmt, "after");
        assert_eq!(leading.attached_to, stmt.span().start);
        assert!(leading.is_leading);
        assert_eq!(trailing.attached_to, stmt.span().start);
        assert!(!trailing.is_leading);

        let block = synth.block(vec![], None);
        let dangling = SyntheticComment::dangling(&block, "unreachable");
        assert_eq!(dangling.attached_to, block.span.start);
    }

    #[test]
    fn version_pinned_field_names() {
        // Unpinned: `goto` is a keyword somewhere, so it gets bracketed.
        let unpinned = Synth::new();
        assert!(matches!(
            unpinned.field_or_index(unpinned.name_expr("t"), "goto"),
            Expression::Var(Var::Index(_))
        ));

        // Pinned to Luau: `goto` is a plain identifier there.
        let luau = Synth::new().with_version(LuaVersion::Luau);
        assert!(matches!(
            luau.field_or_index(luau.name_expr("t"), "goto"),
            Expression::Var(Var::FieldAccess(_))
        ));

        // Pinned to 5.4: still a keyword, still bracketed.
        let lua54 = Synth::new().with_version(LuaVersion::Lua54);
        assert!(matches!(
            lua54.field_or_index(lua54.name_expr("t"), "goto"),
            Expression::Var(Var::Index(_))
        ));
    }

    #[test]
    fn cast_operand_parenthesized_before_type_extending_ops() {
        let synth = Synth::new();
        // (a :: T) < b: bare, the type parser reads `<` as generic args.
        for op in [BinOp::Lt, BinOp::BitAnd, BinOp::BitOr] {
            let cast = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
            let compared = synth.binary_op(cast, op, synth.name_expr("b"));
            let Expression::BinaryOp(binary_op) = &compared else {
                panic!("expected binop");
            };
            assert!(
                matches!(binary_op.left, Expression::Parenthesized(_)),
                "cast before {op:?} must be parenthesized"
            );
        }

        // a + b :: T stays bare: `+` can't extend a printed type.
        let right_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let bare_sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, right_cast);
        let Expression::BinaryOp(bare_node) = &bare_sum else {
            panic!("expected binop");
        };
        assert!(matches!(bare_node.right, Expression::TypeCast(_)));

        // The cast may sit at the end of a larger left operand: the whole
        // operand gains parens, `(a + b :: T) < c`.
        let trailing_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let cast_sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, trailing_cast);
        let sum_compared = synth.binary_op(cast_sum, BinOp::Lt, synth.name_expr("c"));
        let Expression::BinaryOp(sum_node) = &sum_compared else {
            panic!("expected binop");
        };
        assert!(matches!(sum_node.left, Expression::Parenthesized(_)));

        // A cast on the right of `<` ends the expression; no parens needed.
        let rhs_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let cast_compared = synth.binary_op(synth.name_expr("a"), BinOp::Lt, rhs_cast);
        let Expression::BinaryOp(compared_node) = &cast_compared else {
            panic!("expected binop");
        };
        assert!(matches!(compared_node.right, Expression::TypeCast(_)));
    }

    #[test]
    fn chained_cast_parenthesized() {
        let synth = Synth::new();
        // (a :: T) :: U: `a :: T :: U` is a parse error in Luau.
        let inner = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
        let outer = synth.type_cast(inner, synth.ty_named("U"));
        let Expression::TypeCast(cast) = &outer else {
            panic!("expected type cast");
        };
        assert!(matches!(cast.expr, Expression::Parenthesized(_)));
    }

    #[test]
    fn single_value_wraps_multi_value_forms() {
        let synth = Synth::new();
        let call = synth.call(synth.name_expr("f"), vec![]);
        assert!(matches!(synth.single_value(call), Expression::Parenthesized(_)));
        assert!(matches!(synth.single_value(synth.vararg()), Expression::Parenthesized(_)));
        // Single-value expressions pass through unchanged.
        assert!(matches!(synth.single_value(synth.name_expr("x")), Expression::Var(_)));
        assert!(matches!(synth.single_value(synth.number_i64(1)), Expression::Number(_)));
    }

    #[test]
    fn long_string_carriage_return_falls_back_to_quoted() {
        let synth = Synth::new();
        let Expression::StringLiteral(literal) = synth.long_string("a\r\nb") else {
            panic!("expected string literal");
        };
        assert_eq!(literal.text.as_str(), "\"a\\r\\nb\"");
    }

    #[test]
    fn number_f64_prefers_exponent_when_shorter() {
        let synth = Synth::new();
        assert_eq!(number_text(synth.number_f64(1e300)), "1e300");

        assert_eq!(number_text(synth.number_f64(100.0)), "1e2");

        // Plain forms win ties.
        assert_eq!(number_text(synth.number_f64(3.0)), "3.0");
        assert_eq!(number_text(synth.number_f64(1.5)), "1.5");
    }

    #[test]
    fn number_rendering_respects_pinned_number_model() {
        // Plain digits win ties; exponent notation wins when shorter.
        let luau = Synth::new().with_version(LuaVersion::Luau);
        let renderings: [(f64, &str); 6] = [
            (100.0, "100"),
            (1000.0, "1e3"),
            (10000.0, "1e4"),
            (3.0, "3"),
            (1.5, "1.5"),
            (1e300, "1e300"),
        ];
        for (value, expected) in renderings {
            assert_eq!(number_text(luau.number_f64(value)), expected);
        }
        assert!(matches!(luau.number_f64(-0.0), Expression::UnaryOp(_)));

        // Hex would lex as +2^63 on Luau; the decimal magnitude is exact
        // as a float, so i64::MIN becomes its negation.
        let Expression::UnaryOp(negated) = luau.number_i64(i64::MIN) else {
            panic!("expected unop");
        };
        assert_eq!(number_text(negated.operand), "9223372036854775808");

        // The integer/float split keeps the markers on 5.3+.
        let lua54 = Synth::new().with_version(LuaVersion::Lua54);
        assert_eq!(number_text(lua54.number_f64(3.0)), "3.0");
        assert_eq!(number_text(lua54.number_f64(100.0)), "1e2");
        assert_eq!(number_text(lua54.number_i64(i64::MIN)), "0x8000000000000000");
    }

    #[test]
    fn nan_encoding_is_deliberately_not_bit_exact() {
        let synth = Synth::new();
        for bits in [
            0x7ff8_0000_0000_0000,
            0xfff8_0000_0000_0000,
            0x7ff8_0000_dead_beef,
            0x7ff0_0000_0000_0001,
        ] {
            let Expression::BinaryOp(division) = synth.number_f64(f64::from_bits(bits)) else {
                panic!("expected NaN expression");
            };
            assert_eq!(division.op, BinOp::Div);
            assert!(matches!(&division.left, Expression::Number(literal) if literal.text == "0"));
            assert!(matches!(&division.right, Expression::Number(literal) if literal.text == "0"));
        }
    }

    #[test]
    fn ordinary_luau_numbers_and_exact_integers_are_distinct() {
        let synth = Synth::new().with_version(LuaVersion::Luau);
        // 2^53 + 1, the first integer a double can't hold.
        let value = 0x20_0000_0000_0001;
        let number = number_text(synth.number_i64(value));
        assert_eq!(number, "9007199254740993");
        assert_eq!(number.parse::<f64>().expect("double").to_string(), "9007199254740992");
        let Expression::Integer(integer) = synth.integer_i64(value) else {
            panic!("exact integer")
        };
        assert_eq!(integer.text, "9007199254740993i");
    }

    #[test]
    fn builds_const_declarations() {
        let synth = Synth::new();
        let stmt = synth.const_local(
            vec![synth.attributed_name("frozen", None, None)],
            vec![synth.number_i64(1)],
        );
        let Statement::LocalAssignment(const_local) = &stmt else {
            panic!("expected local assignment");
        };
        assert!(const_local.is_const);

        let plain = synth.local(&["thawed"], vec![synth.number_i64(2)]);
        let Statement::LocalAssignment(plain_local) = &plain else {
            panic!("expected local assignment");
        };
        assert!(!plain_local.is_const);

        let function_stmt = synth.const_function(
            "pinned",
            Vec::new(),
            FunctionSignature::default(),
            synth.block(vec![], None),
        );
        let Statement::LocalFunction(local_function) = &function_stmt else {
            panic!("expected local function");
        };
        assert!(local_function.is_const);
    }

    #[test]
    fn builds_attribute_with_args() {
        let synth = Synth::new();
        let attribute = synth.function_attribute("deprecated", Some(vec![synth.string("use y")]));
        let args = attribute.args.as_ref().expect("args present");
        assert_eq!(args.len(), 1);

        let plain = synth.function_attribute("native", None);
        assert!(plain.args.is_none());
    }

    #[test]
    fn builds_luau_integers_instantiations_and_value_exports() {
        let synth = Synth::new().with_version(LuaVersion::Luau);

        let Expression::Integer(integer) = synth.integer_i64(-1) else {
            panic!("expected integer literal");
        };
        assert_eq!(integer.text.as_str(), "0xffffffffffffffffi");

        let Expression::TypeInstantiation(instantiated) =
            synth.type_instantiation(synth.name_expr("f"), vec![synth.ty_named("integer")])
        else {
            panic!("expected type instantiation");
        };
        assert_eq!(instantiated.type_args.args.len(), 1);

        let Expression::FunctionCall(method) = synth.method_call_with_types(
            synth.name_expr("receiver"),
            "map",
            vec![synth.ty_named("string")],
            vec![synth.string("value")],
        ) else {
            panic!("expected method call");
        };
        assert_eq!(method.explicit_type_args.expect("type arguments").args.len(), 1);

        let exported = synth.export_local(
            vec![synth.attributed_name("value", Some(synth.ty_named("integer")), None)],
            vec![synth.integer_i64(1)],
        );
        assert!(matches!(
            exported,
            Statement::LocalAssignment(local) if local.is_exported
        ));
    }
}
