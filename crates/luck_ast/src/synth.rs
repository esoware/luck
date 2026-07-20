//! Programmatic AST synthesis.
//!
//! Builds `luck_ast` nodes without any source text. Hand-writing the node
//! structs means spelling out every `Token { kind, span }`; this module hides
//! that behind small constructors. The intended consumer is a tool that emits
//! an AST directly - so the constructors also take over the output-validity duties
//! such a tool would otherwise have to reimplement.
//!
//! # Spans
//!
//! Every synthesized node gets a fresh single-point span `Span::new(n, n)`
//! with `n` strictly increasing. The spans do not index into any source -
//! they exist only to keep nodes distinguishable for comment anchoring and
//! diagnostics. Spans are unique only within one `Synth`; when several
//! synthesizers feed one tree (parallel decompilation of function protos, or
//! splicing synthetic nodes into a parsed AST), give each a disjoint range
//! via [`Synth::starting_at`] or comment anchors will collide.
//!
//! # What the constructors guarantee
//!
//! - **Operator precedence.** [`Synth::binop`], [`Synth::unop`], and
//!   [`Synth::type_cast`] parenthesize their operands whenever the printed
//!   form would re-parse differently: precedence and associativity for
//!   nested binary operators, `(-a)^b`, greedy Luau if-expressions,
//!   `(a + b) :: T`, chained casts (`(a :: T) :: U`), and casts printed
//!   before `<`, `&`, or `|`, which the type grammar would otherwise
//!   swallow (`(a :: T) < b`).
//! - **Prefix positions.** Call callees, method receivers, and index/field
//!   prefixes that are not already a var, call, or parenthesized expression
//!   are wrapped: `("s"):rep(2)`, `({}).x`, `(function() end)()`.
//! - **Escaping.** [`Synth::string`] and [`Synth::string_bytes`] produce the
//!   quoted token text (decimal escapes are always three digits so a
//!   following literal digit cannot extend them); interpolated-string
//!   segments escape `` ` ``, `{`, and `\`. [`Synth::long_string`] falls
//!   back to the quoted form for content long brackets cannot carry (`\r`).
//! - **Numeric literals.** [`Synth::number_f64`] and [`Synth::number_int`]
//!   render any value, including the ones with no literal form: negatives
//!   (unary minus node), infinities (`1/0`), NaN (`0/0`), and `i64::MIN`
//!   (hex, which wraps to the intended integer on Lua 5.3+). Magnitudes
//!   whose plain decimal form is long render in exponent form (`1e300`).
//! - **Loud failure on invalid names.** [`Synth::ident`] asserts (in release
//!   builds too) that its argument is an identifier, so hostile input -
//!   bytecode debug info, obfuscated names - fails fast instead of emitting
//!   output that will not re-parse.
//!
//! # What the caller still owes
//!
//! - **Multi-value truncation.** `f(g())` spreads all of `g`'s results;
//!   pass the expression through [`Synth::single_value`] when exactly one
//!   value is meant (the last argument of a call, return, or assignment
//!   list). Only the caller knows which positions need it.
//! - **Version gating.** Nothing stops synthesis of, say, a Lua 5.4
//!   attribute for a Luau target; pick constructors matching the dialect.
//!   [`Synth::with_version`] pins name validity and field-name safety to
//!   one dialect but deliberately does not gate constructs.
//! - **Identifier validity.** For names that may not be identifiers
//!   (bytecode debug info), route fields through [`Synth::field_or_index`]
//!   and keys through [`Synth::record`], which fall back to bracketed
//!   string form; every other name position (locals, params, labels,
//!   function and method names) has no bracketed spelling, so pre-check
//!   with [`is_valid_identifier`] / [`is_valid_identifier_in`] and rename.
//!
//! Formatting the result is `luck_formatter::format_block`, which accepts
//! `Comments::synthetic` for [`SyntheticComment`] attachment; compact output
//! is `luck_codegen` with an empty source string.
//!
//! # Usage
//!
//! ```
//! use luck_ast::synth::Synth;
//! use luck_ast::{Expression, Statement};
//! use luck_token::BinOp;
//!
//! let synth = Synth::new();
//! // total = a + b * 2 -- precedence handled, no manual parens.
//! let sum = synth.binop(
//!     synth.name_expr("a"),
//!     BinOp::Add,
//!     synth.binop(synth.name_expr("b"), BinOp::Mul, synth.number_int(2)),
//! );
//! let stmt = synth.assign(vec![synth.name_var("total")], vec![sum]);
//! let block = synth.block(vec![stmt], None);
//! assert_eq!(block.stmts.len(), 1);
//! ```

use std::cell::Cell;

use luck_token::{
    Assoc, BinOp, CompactString, CompoundOp, LuaVersion, Span, Token, TokenKind, UNARY_PRECEDENCE,
    UnOp,
};

use crate::expr::*;
use crate::shared::*;
use crate::stmt::*;
use crate::types::*;

/// Monotonic span allocator + node constructors. Methods take `&self` so
/// calls nest: `synth.call(synth.name_expr("f"), vec![synth.nil()])`.
#[derive(Debug, Default)]
pub struct Synth {
    next_offset: Cell<u32>,
    version: Option<LuaVersion>,
}

/// A table field passed to [`Synth::table`], before the separators and spans
/// the real [`Field`] carries are filled in.
pub enum SynthField<'a> {
    Positional(Expression),
    Named(&'a str, Expression),
    Bracketed(Expression, Expression),
}

/// One piece of [`Synth::interpolated_string`], in source order: literal
/// text (UNESCAPED) or an interpolated `{expr}`.
pub enum SynthInterpPart<'a> {
    Text(&'a str),
    Expr(Expression),
}

/// A table-type field passed to [`Synth::ty_table`], before the separators
/// and spans the real [`TypeField`] carries are filled in.
pub enum SynthTypeField<'a> {
    Named {
        access: Option<TypeFieldAccess>,
        name: &'a str,
        value: Type,
    },
    Indexer {
        access: Option<TypeFieldAccess>,
        key: Type,
        value: Type,
    },
    Array(Type),
}

/// Luau `read`/`write` access modifier on a table-type field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFieldAccess {
    Read,
    Write,
}

/// Full function signature for the `*_full` constructors. `Default` is an
/// empty, untyped `()` signature; fill only the fields you need.
#[derive(Debug, Default)]
pub struct FnSig {
    /// Luau `<T, U...>` list before the parameter parens.
    pub generics: Option<GenericTypeList>,
    pub params: Vec<Parameter>,
    /// Trailing `...`, optionally typed - see [`Synth::vararg_param`].
    pub vararg: Option<VarArgParam>,
    /// Luau `: T` return annotation.
    pub return_type: Option<Type>,
}

/// A comment to attach during a later formatting pass. Comments are not in the
/// AST (they live in a side table keyed by `attached_to`), so synthesis returns
/// this as dumb data for the formatter to consume; nothing here interprets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticComment {
    pub attached_to: u32,
    pub text: CompactString,
    pub is_leading: bool,
}

/// Whether `name` lexes as an identifier in at least one supported dialect:
/// ASCII identifier shape and not one of the 21 always-reserved keywords.
/// `goto`, `global`, and the Luau context-sensitive words (`type`,
/// `continue`, `export`) pass. For a single-dialect check, use
/// [`is_valid_identifier_in`].
#[must_use]
pub fn is_valid_identifier(name: &str) -> bool {
    is_identifier_shaped(name) && !is_base_keyword(name)
}

/// Whether `name` lexes as an identifier under `version`: identifier shape
/// and not reserved there. `goto` is reserved on Lua 5.2-5.5 but is an
/// identifier on 5.1 and Luau; `global` is reserved only on 5.5.
#[must_use]
pub fn is_valid_identifier_in(name: &str, version: LuaVersion) -> bool {
    is_identifier_shaped(name) && !is_keyword_in(name, version)
}

fn is_identifier_shaped(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_keyword_in(name: &str, version: LuaVersion) -> bool {
    is_base_keyword(name)
        || (name == "goto" && version.has_goto())
        || (name == "global" && version.has_global())
}

fn is_base_keyword(name: &str) -> bool {
    matches!(
        name,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
    )
}

/// Operand position relative to a binary operator.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// Whether the type grammar consumes this operator's token when it directly
/// follows a printed type: `<` opens generic arguments after a named type,
/// `|` and `&` continue a union or intersection.
fn op_token_extends_types(op: BinOp) -> bool {
    matches!(op, BinOp::Lt | BinOp::BitAnd | BinOp::BitOr)
}

/// Whether `expr` prints with a type cast as its rightmost part, so the next
/// token lands directly after the cast's type.
fn ends_with_type_cast(expr: &Expression) -> bool {
    match expr {
        Expression::TypeCast(_) => true,
        Expression::BinaryOp(inner) => ends_with_type_cast(&inner.right),
        Expression::UnaryOp(inner) => ends_with_type_cast(&inner.operand),
        _ => false,
    }
}

impl Synth {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A synthesizer whose spans start at `offset`. Use disjoint ranges when
    /// several synthesizers contribute to one tree, or when synthetic nodes
    /// are spliced into a parsed AST whose byte offsets they must not shadow.
    #[must_use]
    pub fn starting_at(offset: u32) -> Self {
        Self {
            next_offset: Cell::new(offset),
            version: None,
        }
    }

    /// Pin name handling to one dialect: [`Synth::ident`] asserts validity
    /// against that dialect's keyword set instead of the cross-dialect union,
    /// and [`Synth::field_or_index`] / [`Synth::record`] use the dot/named
    /// form for names that are identifiers there (`t.goto` on Luau) instead
    /// of the everywhere-safe bracketed fallback. Construct availability is
    /// deliberately not gated.
    #[must_use]
    pub fn with_version(mut self, version: LuaVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// A fresh single-point span. Distinct per node; never slices source.
    fn next_span(&self) -> Span {
        let offset = self.next_offset.get();
        self.next_offset.set(offset + 1);
        Span::new(offset, offset)
    }

    #[must_use]
    pub fn token(&self, kind: TokenKind) -> Token {
        Token::new(kind, self.next_span())
    }

    /// An identifier token. Asserts validity - in release builds too, so
    /// untrusted names (bytecode debug info) fail loudly instead of emitting
    /// output that will not re-parse. For names that may not be identifiers,
    /// use [`Synth::field_or_index`] / [`Synth::record`] / [`Synth::string`]
    /// instead of forcing them through here.
    #[must_use]
    pub fn ident(&self, name: &str) -> Token {
        let is_valid = match self.version {
            Some(version) => is_valid_identifier_in(name, version),
            None => is_valid_identifier(name),
        };
        assert!(is_valid, "synth: {name:?} is not a valid identifier");
        self.token(TokenKind::Identifier(CompactString::from(name)))
    }

    /// Identifier-shaped and safe as a `.name` field / `name =` key: under a
    /// pinned version, exactly that dialect's identifiers; otherwise the
    /// conservative set that works in every dialect (excluding `goto` and
    /// `global`, identifiers in some dialects and keywords in others).
    fn is_safe_field_name(&self, name: &str) -> bool {
        match self.version {
            Some(version) => is_valid_identifier_in(name, version),
            None => is_valid_identifier(name) && !matches!(name, "goto" | "global"),
        }
    }

    #[must_use]
    pub fn name_var(&self, name: &str) -> Var {
        Var::Name(self.ident(name))
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
            name: self.ident(name),
        }))
    }

    #[must_use]
    pub fn var_expr(&self, var: Var) -> Expression {
        Expression::Var(var)
    }

    #[must_use]
    pub fn name_expr(&self, name: &str) -> Expression {
        self.var_expr(self.name_var(name))
    }

    #[must_use]
    pub fn index(&self, prefix: Expression, index: Expression) -> Expression {
        self.var_expr(self.index_var(prefix, index))
    }

    #[must_use]
    pub fn field(&self, prefix: Expression, name: &str) -> Expression {
        self.var_expr(self.field_var(prefix, name))
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
    /// [`Synth::number_f64`] / [`Synth::number_int`] for runtime values.
    #[must_use]
    pub fn number(&self, text: &str) -> Expression {
        Expression::Number(Literal {
            text: CompactString::from(text),
            span: self.next_span(),
        })
    }

    /// Any `f64` as an expression that evaluates back to exactly `value`.
    /// Finite integral values render with a `.0` suffix so the float subtype
    /// survives on Lua 5.3+ (use [`Synth::number_int`] where an integer is
    /// meant); negatives become a unary-minus node, infinities `1/0`, NaN
    /// `0/0`, and magnitudes whose plain decimal form is longer render in
    /// exponent form (`1e300`, not a 300-digit literal).
    #[must_use]
    pub fn number_f64(&self, value: f64) -> Expression {
        if value.is_nan() {
            return self.binop(self.number("0"), BinOp::Div, self.number("0"));
        }
        if value.is_infinite() {
            let inf = self.binop(self.number("1"), BinOp::Div, self.number("0"));
            return if value < 0.0 {
                self.unop(UnOp::Neg, inf)
            } else {
                inf
            };
        }
        if value.is_sign_negative() {
            // Lua has no negative literals; this also keeps -0.0's sign bit.
            return self.unop(UnOp::Neg, self.number_f64(-value));
        }
        // Display never uses exponent notation, so it only ever yields
        // digits and a possible decimal point.
        let mut text = value.to_string();
        if !text.contains('.') {
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

    /// Any `i64` as an expression that evaluates back to exactly `value`.
    #[must_use]
    pub fn number_int(&self, value: i64) -> Expression {
        if value == i64::MIN {
            // The magnitude overflows a Lua 5.3+ integer literal and would
            // silently become a float; the hex form wraps to the intended value.
            return self.number("0x8000000000000000");
        }
        if value < 0 {
            return self.unop(UnOp::Neg, self.number(&(-value).to_string()));
        }
        self.number(&value.to_string())
    }

    /// String literal from UNQUOTED content; the stored token text is the raw
    /// double-quoted form.
    #[must_use]
    pub fn string(&self, content: &str) -> Expression {
        Expression::StringLiteral(Literal {
            text: quote_utf8(content),
            span: self.next_span(),
        })
    }

    /// String literal from arbitrary bytes - Lua strings are byte arrays and
    /// bytecode constants need not be UTF-8. Bytes outside printable ASCII
    /// render as decimal escapes, so the token text is always valid UTF-8.
    #[must_use]
    pub fn string_bytes(&self, content: &[u8]) -> Expression {
        Expression::StringLiteral(Literal {
            text: quote_bytes(content),
            span: self.next_span(),
        })
    }

    /// `[[...]]` long-bracket string, picking the smallest `=` level whose
    /// closer cannot occur early. Long brackets cannot represent `\r` (Lua
    /// normalizes line endings inside them), so such content falls back to
    /// the escaped quoted form of [`Synth::string`].
    #[must_use]
    pub fn long_string(&self, content: &str) -> Expression {
        if content.contains('\r') {
            return self.string(content);
        }
        let mut level = 0;
        let equals = loop {
            let equals = "=".repeat(level);
            let closer = format!("]{equals}]");
            // At level 0 a trailing `]` merges with the closer one byte early.
            let collides = content.contains(&closer) || (level == 0 && content.ends_with(']'));
            if !collides {
                break equals;
            }
            level += 1;
        };
        let mut text = String::with_capacity(content.len() + 2 * equals.len() + 5);
        text.push('[');
        text.push_str(&equals);
        text.push('[');
        // Lua swallows a newline directly after the opening bracket.
        if content.starts_with('\n') {
            text.push('\n');
        }
        text.push_str(content);
        text.push(']');
        text.push_str(&equals);
        text.push(']');
        Expression::StringLiteral(Literal {
            text: CompactString::from(text),
            span: self.next_span(),
        })
    }

    #[must_use]
    pub fn nil(&self) -> Expression {
        Expression::Nil(self.next_span())
    }

    #[must_use]
    pub fn boolean(&self, value: bool) -> Expression {
        if value {
            Expression::True(self.next_span())
        } else {
            Expression::False(self.next_span())
        }
    }

    #[must_use]
    pub fn vararg(&self) -> Expression {
        Expression::VarArg(self.next_span())
    }

    /// `lhs op rhs`, parenthesizing either operand when precedence,
    /// associativity, a greedy if-expression, or a type cast printed before
    /// a type-extending operator token would otherwise change the parse.
    #[must_use]
    pub fn binop(&self, lhs: Expression, op: BinOp, rhs: Expression) -> Expression {
        let (precedence, assoc) = op.precedence();
        let lhs = self.wrap_binop_operand(lhs, op, precedence, assoc, Side::Left);
        let rhs = self.wrap_binop_operand(rhs, op, precedence, assoc, Side::Right);
        Expression::BinaryOp(Box::new(BinaryOp {
            span: self.next_span(),
            left: lhs,
            op,
            right: rhs,
        }))
    }

    /// `op operand`, parenthesizing a binary or if-expression operand.
    #[must_use]
    pub fn unop(&self, op: UnOp, operand: Expression) -> Expression {
        let operand = match operand {
            wrapped @ (Expression::BinaryOp(_) | Expression::IfExpression(_)) => {
                self.paren(wrapped)
            }
            other => other,
        };
        Expression::UnaryOp(Box::new(UnaryOp {
            span: self.next_span(),
            op,
            operand,
        }))
    }

    #[must_use]
    pub fn paren(&self, expr: Expression) -> Expression {
        Expression::Parenthesized(Box::new(ParenExpression {
            span: self.next_span(),
            expr,
        }))
    }

    /// Truncate a multi-value expression to exactly one value: calls and
    /// `...` (the only multi-value forms) are parenthesized, everything else
    /// passes through untouched. Use in single-result positions - the last
    /// expression of a call, return, or assignment list where exactly one
    /// value is meant (a bytecode `CALL` requesting one result).
    #[must_use]
    pub fn single_value(&self, expr: Expression) -> Expression {
        match expr {
            multi @ (Expression::FunctionCall(_) | Expression::VarArg(_)) => self.paren(multi),
            single => single,
        }
    }

    /// Luau type cast: `expr :: T`. The cast binds to a simple expression, so
    /// binary, unary, and if-expression operands are parenthesized - as are
    /// cast operands, because chained casts (`a :: T :: U`) are a parse error
    /// in Luau.
    #[must_use]
    pub fn type_cast(&self, expr: Expression, type_value: Type) -> Expression {
        let expr = match expr {
            wrapped @ (Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_)) => self.paren(wrapped),
            other => other,
        };
        Expression::TypeCast(Box::new(TypeCast {
            span: self.next_span(),
            expr,
            type_annotation: type_value,
        }))
    }

    fn wrap_binop_operand(
        &self,
        operand: Expression,
        parent_op: BinOp,
        parent_precedence: u8,
        parent_assoc: Assoc,
        side: Side,
    ) -> Expression {
        let needs_parens = match &operand {
            Expression::BinaryOp(inner) => {
                let (inner_precedence, _) = inner.op.precedence();
                inner_precedence < parent_precedence
                    || (inner_precedence == parent_precedence
                        && parent_assoc
                            == match side {
                                Side::Left => Assoc::Right,
                                Side::Right => Assoc::Left,
                            })
            }
            // Only `^` binds tighter than unary; `a ^ -b` needs no parens
            // because a unary operator is always accepted after a binop.
            Expression::UnaryOp(_) => side == Side::Left && parent_precedence > UNARY_PRECEDENCE,
            // An if-expression extends greedily rightward; anything printed
            // after one would be swallowed into its else branch. Wrapping on
            // both sides keeps that true for every later composition too.
            Expression::IfExpression(_) => true,
            _ => false,
        } || (side == Side::Left
            && op_token_extends_types(parent_op)
            && ends_with_type_cast(&operand));
        if needs_parens {
            self.paren(operand)
        } else {
            operand
        }
    }

    /// Parenthesize expressions that cannot stand in prefix position
    /// (call callee, method receiver, index/field prefix).
    fn wrap_prefix(&self, prefix: Expression) -> Expression {
        match prefix {
            Expression::Var(_) | Expression::FunctionCall(_) | Expression::Parenthesized(_) => {
                prefix
            }
            other => self.paren(other),
        }
    }

    #[must_use]
    pub fn call(&self, callee: Expression, args: Vec<Expression>) -> Expression {
        let args = self.paren_args(args);
        self.call_node(callee, None, args)
    }

    #[must_use]
    pub fn method_call(
        &self,
        receiver: Expression,
        name: &str,
        args: Vec<Expression>,
    ) -> Expression {
        let args = self.paren_args(args);
        self.call_node(receiver, Some(name), args)
    }

    /// `callee{ fields }` - table-constructor argument form.
    #[must_use]
    pub fn call_table(&self, callee: Expression, fields: Vec<SynthField<'_>>) -> Expression {
        let args = FunctionArgs::TableConstructor(Box::new(self.table_ctor(fields)));
        self.call_node(callee, None, args)
    }

    /// `callee"content"` - string argument form (content is unquoted).
    #[must_use]
    pub fn call_string(&self, callee: Expression, content: &str) -> Expression {
        let args = FunctionArgs::StringLiteral(Literal {
            text: quote_utf8(content),
            span: self.next_span(),
        });
        self.call_node(callee, None, args)
    }

    /// `receiver:name{ fields }`.
    #[must_use]
    pub fn method_call_table(
        &self,
        receiver: Expression,
        name: &str,
        fields: Vec<SynthField<'_>>,
    ) -> Expression {
        let args = FunctionArgs::TableConstructor(Box::new(self.table_ctor(fields)));
        self.call_node(receiver, Some(name), args)
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
        self.call_node(receiver, Some(name), args)
    }

    fn call_node(
        &self,
        callee: Expression,
        method: Option<&str>,
        args: FunctionArgs,
    ) -> Expression {
        Expression::FunctionCall(Box::new(FunctionCall {
            span: self.next_span(),
            callee: self.wrap_prefix(callee),
            args,
            method: method.map(|name| self.ident(name)),
        }))
    }

    #[must_use]
    pub fn table(&self, fields: Vec<SynthField<'_>>) -> Expression {
        Expression::TableConstructor(Box::new(self.table_ctor(fields)))
    }

    /// `{ v1, v2, ... }` - all-positional table.
    #[must_use]
    pub fn array(&self, values: Vec<Expression>) -> Expression {
        self.table(values.into_iter().map(SynthField::Positional).collect())
    }

    /// `{ k = v, ... }` - named-field table. Keys that are not safe
    /// identifiers fall back to the bracketed string form.
    #[must_use]
    pub fn record(&self, fields: Vec<(&str, Expression)>) -> Expression {
        self.table(
            fields
                .into_iter()
                .map(|(name, value)| {
                    if self.is_safe_field_name(name) {
                        SynthField::Named(name, value)
                    } else {
                        SynthField::Bracketed(self.string(name), value)
                    }
                })
                .collect(),
        )
    }

    fn table_ctor(&self, fields: Vec<SynthField<'_>>) -> TableConstructor {
        let span = self.next_span();
        let built = fields
            .into_iter()
            .map(|field| match field {
                SynthField::Positional(value) => Field::Positional {
                    span: self.next_span(),
                    value,
                },
                SynthField::Named(name, value) => Field::Named {
                    span: self.next_span(),
                    name: self.ident(name),
                    value,
                },
                SynthField::Bracketed(key, value) => Field::Bracketed {
                    span: self.next_span(),
                    key,
                    value,
                },
            })
            .collect();
        TableConstructor {
            span,
            fields: Punctuated::from_items(built),
        }
    }

    /// `function(params) body end` with plain untyped parameters. For
    /// attributes, generics, typed parameters, varargs, or a return type,
    /// use [`Synth::function_def_full`].
    #[must_use]
    pub fn function_def(&self, params: &[&str], body: Block) -> Expression {
        self.function_def_full(
            Vec::new(),
            FnSig {
                params: self.params(params),
                ..FnSig::default()
            },
            body,
        )
    }

    /// Full-fidelity anonymous function. Build `attributes` with
    /// [`Synth::function_attribute`].
    #[must_use]
    pub fn function_def_full(
        &self,
        attributes: Vec<FunctionAttribute>,
        sig: FnSig,
        body: Block,
    ) -> Expression {
        Expression::FunctionDef(Box::new(FunctionDef {
            span: self.next_span(),
            attributes,
            body: self.function_body(sig, body),
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
            FnSig {
                params: self.params(params),
                ..FnSig::default()
            },
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
        sig: FnSig,
        body: Block,
    ) -> Statement {
        Statement::FunctionDecl(Box::new(FunctionDecl {
            span: self.next_span(),
            attributes,
            name: self.func_name(name_path, method),
            body: self.function_body(sig, body),
        }))
    }

    /// `local function name(params) body end`. For attributes or a full
    /// signature, use [`Synth::local_function_full`].
    #[must_use]
    pub fn local_function(&self, name: &str, params: &[&str], body: Block) -> Statement {
        self.local_function_full(
            name,
            Vec::new(),
            FnSig {
                params: self.params(params),
                ..FnSig::default()
            },
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
        sig: FnSig,
        body: Block,
    ) -> Statement {
        self.local_function_node(name, attributes, sig, body, false)
    }

    /// Luau `const function name(params) body end` - a read-only local
    /// function binding.
    #[must_use]
    pub fn const_function(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        sig: FnSig,
        body: Block,
    ) -> Statement {
        self.local_function_node(name, attributes, sig, body, true)
    }

    fn local_function_node(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        sig: FnSig,
        body: Block,
        is_const: bool,
    ) -> Statement {
        Statement::LocalFunction(Box::new(LocalFunction {
            span: self.next_span(),
            attributes,
            name: self.ident(name),
            body: self.function_body(sig, body),
            is_const,
        }))
    }

    /// Lua 5.5 `global function name(params) body end`.
    #[must_use]
    pub fn global_function(&self, name: &str, sig: FnSig, body: Block) -> Statement {
        Statement::GlobalFunction(Box::new(GlobalFunction {
            span: self.next_span(),
            name: self.ident(name),
            body: self.function_body(sig, body),
        }))
    }

    fn func_name(&self, name_path: &[&str], method: Option<&str>) -> FuncName {
        let names: Vec<Token> = name_path.iter().map(|part| self.ident(part)).collect();
        FuncName {
            span: self.next_span(),
            names,
            method: method.map(|name| self.ident(name)),
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
            name: self.ident(name),
            args: args.map(|args| self.punctuated(args)),
        }
    }

    fn function_body(&self, sig: FnSig, block: Block) -> FunctionBody {
        FunctionBody {
            span: self.next_span(),
            generics: sig.generics.map(Box::new),
            params: self.punctuated(sig.params),
            vararg: sig.vararg,
            return_type: sig.return_type,
            block,
        }
    }

    /// Luau backtick string from literal text and interpolated expressions
    /// in source order: `` `hi {name}!` `` is
    /// `vec![Text("hi "), Expr(name), Text("!")]`. Text is UNESCAPED; `` ` ``,
    /// `{`, and `\` are escaped here. Adjacent text parts merge, and the
    /// lexer's `InterpBegin`/`InterpMid`/`InterpEnd` segment shape (the
    /// terminal segment carries the trailing text with no expression) is
    /// derived internally.
    #[must_use]
    pub fn interpolated_string(&self, parts: Vec<SynthInterpPart<'_>>) -> Expression {
        let span = self.next_span();
        let mut pending: Vec<(CompactString, Option<Expression>)> = Vec::new();
        let mut text = String::new();
        for part in parts {
            match part {
                SynthInterpPart::Text(more) => text.push_str(more),
                SynthInterpPart::Expr(expr) => {
                    pending.push((interp_text(&text), Some(expr)));
                    text.clear();
                }
            }
        }
        if pending.is_empty() {
            // Plain-text form: the lexer shape is InterpBegin("") followed
            // by InterpEnd(text).
            pending.push((CompactString::from(""), None));
        }
        pending.push((interp_text(&text), None));
        let last_index = pending.len() - 1;
        let segments: Vec<InterpSegment> = pending
            .into_iter()
            .enumerate()
            .map(|(index, (segment_text, expr))| {
                let literal = if index == 0 {
                    self.token(TokenKind::InterpBegin(segment_text))
                } else if index == last_index {
                    self.token(TokenKind::InterpEnd(segment_text))
                } else {
                    self.token(TokenKind::InterpMid(segment_text))
                };
                InterpSegment { literal, expr }
            })
            .collect();
        Expression::InterpolatedString(Box::new(InterpolatedString { span, segments }))
    }

    /// Luau `if c then a {elseif c2 then b} else z` expression.
    #[must_use]
    pub fn if_expr(
        &self,
        cond: Expression,
        then_expr: Expression,
        elseifs: Vec<(Expression, Expression)>,
        else_expr: Expression,
    ) -> Expression {
        let span = self.next_span();
        let elseif_clauses: Vec<ElseIfExprClause> = elseifs
            .into_iter()
            .map(|(condition, expr)| ElseIfExprClause {
                span: self.next_span(),
                condition,
                expr,
            })
            .collect();
        Expression::IfExpression(Box::new(IfExpression {
            span,
            condition: cond,
            then_expr,
            elseif_clauses,
            else_expr,
        }))
    }

    /// `local names = exprs` with plain untyped names. For type annotations
    /// or Lua 5.4 attributes, use [`Synth::local_full`].
    #[must_use]
    pub fn local(&self, names: &[&str], exprs: Vec<Expression>) -> Statement {
        let names = names
            .iter()
            .map(|name| self.attributed_name(name, None, None))
            .collect();
        self.local_full(names, exprs)
    }

    #[must_use]
    pub fn local_full(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        self.local_assignment(names, exprs, false)
    }

    /// Luau `const names = exprs` - a `local` whose names are read-only.
    /// Asserts a non-empty initializer list; the grammar requires one.
    #[must_use]
    pub fn const_local(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        assert!(
            !exprs.is_empty(),
            "synth: const declarations require an initializer"
        );
        self.local_assignment(names, exprs, true)
    }

    fn local_assignment(
        &self,
        names: Vec<AttributedName>,
        exprs: Vec<Expression>,
        is_const: bool,
    ) -> Statement {
        let span = self.next_span();
        let names = self.punctuated(names);
        let exprs = if exprs.is_empty() {
            None
        } else {
            Some(self.punctuated(exprs))
        };
        Statement::LocalAssignment(Box::new(LocalAssignment {
            span,
            names,
            exprs,
            is_const,
        }))
    }

    /// One declared name for [`Synth::local_full`] / [`Synth::global_decl`]:
    /// optional Luau type annotation, optional Lua 5.4 attribute keyword
    /// (`"const"` / `"close"`). The two are mutually exclusive in practice.
    #[must_use]
    pub fn attributed_name(
        &self,
        name: &str,
        ty: Option<Type>,
        attrib: Option<&str>,
    ) -> AttributedName {
        AttributedName {
            name: self.ident(name),
            type_annotation: ty,
            attrib: attrib.map(|attribute| Attribute {
                span: self.next_span(),
                name: self.ident(attribute),
            }),
        }
    }

    /// Lua 5.5 `global names [= exprs]` declaration.
    #[must_use]
    pub fn global_decl(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        Statement::GlobalDeclaration(Box::new(GlobalDeclaration {
            span: self.next_span(),
            names: self.punctuated(names),
            exprs: if exprs.is_empty() {
                None
            } else {
                Some(self.punctuated(exprs))
            },
        }))
    }

    /// Lua 5.5 `global *` / `global <attrib> *`.
    #[must_use]
    pub fn global_star(&self, attrib: Option<&str>) -> Statement {
        Statement::GlobalStar(Box::new(GlobalStar {
            span: self.next_span(),
            attrib: attrib.map(|attribute| Attribute {
                span: self.next_span(),
                name: self.ident(attribute),
            }),
        }))
    }

    /// `targets = values`. Build targets with [`Synth::name_var`],
    /// [`Synth::index_var`], [`Synth::field_var`].
    #[must_use]
    pub fn assign(&self, targets: Vec<Var>, values: Vec<Expression>) -> Statement {
        Statement::Assignment(Box::new(Assignment {
            span: self.next_span(),
            targets: self.punctuated(targets),
            values: self.punctuated(values),
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

    /// A call expression as a statement. Panics on a non-call expression.
    #[must_use]
    pub fn call_stmt(&self, call_expr: Expression) -> Statement {
        let call = match call_expr {
            Expression::FunctionCall(call) => *call,
            other => panic!("synth: call_stmt requires a function-call expression, got {other:?}"),
        };
        Statement::FunctionCall(Box::new(FunctionCallStmt {
            span: self.next_span(),
            call,
        }))
    }

    #[must_use]
    pub fn do_block(&self, block: Block) -> Statement {
        Statement::DoBlock(Box::new(DoBlock {
            span: self.next_span(),
            block,
        }))
    }

    #[must_use]
    pub fn while_(&self, cond: Expression, block: Block) -> Statement {
        Statement::WhileLoop(Box::new(WhileLoop {
            span: self.next_span(),
            condition: cond,
            block,
        }))
    }

    #[must_use]
    pub fn repeat_(&self, block: Block, cond: Expression) -> Statement {
        Statement::RepeatLoop(Box::new(RepeatLoop {
            span: self.next_span(),
            block,
            condition: cond,
        }))
    }

    #[must_use]
    pub fn if_(
        &self,
        cond: Expression,
        then_block: Block,
        elseifs: Vec<(Expression, Block)>,
        else_block: Option<Block>,
    ) -> Statement {
        let span = self.next_span();
        let elseif_clauses: Vec<ElseIfClause> = elseifs
            .into_iter()
            .map(|(condition, block)| ElseIfClause {
                span: self.next_span(),
                condition,
                block,
            })
            .collect();
        let else_clause = else_block.map(|block| ElseClause {
            span: self.next_span(),
            block,
        });
        Statement::IfStatement(Box::new(IfStatement {
            span,
            condition: cond,
            block: then_block,
            elseif_clauses,
            else_clause,
        }))
    }

    /// `for var = start, limit [, step] do block end`. Build `var` with
    /// [`Synth::param`] or [`Synth::param_typed`].
    #[must_use]
    pub fn numeric_for(
        &self,
        var: Parameter,
        start: Expression,
        limit: Expression,
        step: Option<Expression>,
        block: Block,
    ) -> Statement {
        Statement::NumericFor(Box::new(NumericFor {
            span: self.next_span(),
            name: var.name,
            type_annotation: var.type_annotation,
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
            names: self.punctuated(names),
            exprs: self.punctuated(exprs),
            block,
        }))
    }

    /// `goto label` (Lua 5.2+).
    #[must_use]
    pub fn goto_(&self, label: &str) -> Statement {
        Statement::Goto(Box::new(GotoStatement {
            span: self.next_span(),
            name: self.ident(label),
        }))
    }

    /// `::name::` label (Lua 5.2+).
    #[must_use]
    pub fn label(&self, name: &str) -> Statement {
        Statement::Label(Box::new(LabelStatement {
            span: self.next_span(),
            name: self.ident(name),
        }))
    }

    /// Mid-block `break` (Lua 5.2+; 5.1 and Luau restrict `break` to the
    /// last statement - use [`Synth::break_`] there).
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
            exprs: self.punctuated(exprs),
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
        Block {
            span: self.next_span(),
            stmts,
            last_stmt: last.map(Box::new),
        }
    }

    #[must_use]
    pub fn param(&self, name: &str) -> Parameter {
        Parameter {
            span: self.next_span(),
            name: self.ident(name),
            type_annotation: None,
        }
    }

    #[must_use]
    pub fn param_typed(&self, name: &str, ty: Type) -> Parameter {
        Parameter {
            span: self.next_span(),
            name: self.ident(name),
            type_annotation: Some(ty),
        }
    }

    /// A trailing `...` parameter, optionally with a Luau pack annotation. The
    /// name stays `None` - Lua 5.5's `...name` form is not synthesized here.
    #[must_use]
    pub fn vararg_param(&self, type_annotation: Option<Type>) -> VarArgParam {
        VarArgParam {
            span: self.next_span(),
            name: None,
            type_annotation,
        }
    }

    fn params(&self, names: &[&str]) -> Vec<Parameter> {
        names.iter().map(|name| self.param(name)).collect()
    }

    #[must_use]
    pub fn ty_named(&self, name: &str) -> Type {
        Type::Named(Box::new(NamedType {
            span: self.next_span(),
            prefix: None,
            name: self.ident(name),
            generics: None,
        }))
    }

    #[must_use]
    pub fn ty_qualified(&self, module: &str, name: &str) -> Type {
        Type::Named(Box::new(NamedType {
            span: self.next_span(),
            prefix: Some(self.ident(module)),
            name: self.ident(name),
            generics: None,
        }))
    }

    #[must_use]
    pub fn ty_generic(&self, name: &str, args: Vec<Type>) -> Type {
        let span = self.next_span();
        let name = self.ident(name);
        let generics = TypeArgs {
            span: self.next_span(),
            args: self.punctuated(args),
        };
        Type::Named(Box::new(NamedType {
            span,
            prefix: None,
            name,
            generics: Some(generics),
        }))
    }

    /// `typeof(expr)`.
    #[must_use]
    pub fn ty_typeof(&self, expr: Expression) -> Type {
        Type::Typeof(Box::new(TypeofType {
            span: self.next_span(),
            expr,
        }))
    }

    #[must_use]
    pub fn ty_optional(&self, inner: Type) -> Type {
        Type::Optional(Box::new(OptionalType {
            span: self.next_span(),
            type_value: inner,
        }))
    }

    #[must_use]
    pub fn ty_union(&self, types: Vec<Type>) -> Type {
        Type::Union(Box::new(UnionType {
            span: self.next_span(),
            has_leading_pipe: false,
            types: self.punctuated(types),
        }))
    }

    #[must_use]
    pub fn ty_intersection(&self, types: Vec<Type>) -> Type {
        Type::Intersection(Box::new(IntersectionType {
            span: self.next_span(),
            has_leading_ampersand: false,
            types: self.punctuated(types),
        }))
    }

    /// `(T)`.
    #[must_use]
    pub fn ty_paren(&self, inner: Type) -> Type {
        Type::Parenthesized(Box::new(ParenType {
            span: self.next_span(),
            type_value: inner,
        }))
    }

    /// `{ T }` array shorthand.
    #[must_use]
    pub fn ty_table_array(&self, element: Type) -> Type {
        self.ty_table(vec![SynthTypeField::Array(element)])
    }

    /// `{ name: T, ... }` named-field table type.
    #[must_use]
    pub fn ty_record(&self, fields: Vec<(&str, Type)>) -> Type {
        self.ty_table(
            fields
                .into_iter()
                .map(|(name, value)| SynthTypeField::Named {
                    access: None,
                    name,
                    value,
                })
                .collect(),
        )
    }

    /// Full table type: named fields, indexers, and the array element in any
    /// mix, each with an optional `read`/`write` access modifier.
    #[must_use]
    pub fn ty_table(&self, fields: Vec<SynthTypeField<'_>>) -> Type {
        let span = self.next_span();
        let built = fields
            .into_iter()
            .map(|field| match field {
                SynthTypeField::Named {
                    access,
                    name,
                    value,
                } => TypeField::Named {
                    span: self.next_span(),
                    access: access.map(|access| self.access_token(access)),
                    name: self.ident(name),
                    value,
                },
                SynthTypeField::Indexer { access, key, value } => TypeField::Indexer {
                    span: self.next_span(),
                    access: access.map(|access| self.access_token(access)),
                    key,
                    value,
                },
                SynthTypeField::Array(value) => TypeField::Array {
                    span: self.next_span(),
                    value,
                },
            })
            .collect();
        Type::Table(Box::new(TableType {
            span,
            fields: Punctuated::from_items(built),
        }))
    }

    fn access_token(&self, access: TypeFieldAccess) -> Token {
        self.ident(match access {
            TypeFieldAccess::Read => "read",
            TypeFieldAccess::Write => "write",
        })
    }

    /// `(T, U) -> R` with unnamed parameters. For generics or named
    /// parameters, use [`Synth::ty_function_full`].
    #[must_use]
    pub fn ty_function(&self, params: Vec<Type>, return_type: Type) -> Type {
        self.ty_function_full(
            None,
            params.into_iter().map(|ty| (None, ty)).collect(),
            return_type,
        )
    }

    /// `<T...>(name: T, U) -> R`. Each parameter is `(optional name, type)`.
    /// A pack return rides in as [`Synth::ty_pack`].
    #[must_use]
    pub fn ty_function_full(
        &self,
        generics: Option<GenericTypeList>,
        params: Vec<(Option<&str>, Type)>,
        return_type: Type,
    ) -> Type {
        let span = self.next_span();
        let params: Vec<FunctionTypeParam> = params
            .into_iter()
            .map(|(name, type_value)| FunctionTypeParam {
                span: self.next_span(),
                name: name.map(|name| self.ident(name)),
                type_value,
            })
            .collect();
        let params = self.punctuated(params);
        Type::Function(Box::new(FunctionType {
            span,
            generics,
            params,
            return_type,
        }))
    }

    #[must_use]
    pub fn ty_singleton_string(&self, content: &str) -> Type {
        Type::Singleton(self.token(TokenKind::StringLiteral(quote_utf8(content))))
    }

    /// `true` / `false` singleton type.
    #[must_use]
    pub fn ty_singleton_bool(&self, value: bool) -> Type {
        let kind = if value {
            TokenKind::True
        } else {
            TokenKind::False
        };
        Type::Singleton(self.token(kind))
    }

    /// `nil` singleton type.
    #[must_use]
    pub fn ty_singleton_nil(&self) -> Type {
        Type::Singleton(self.token(TokenKind::Nil))
    }

    #[must_use]
    pub fn ty_pack(&self, types: Vec<Type>) -> Type {
        let span = self.next_span();
        Type::Pack(Box::new(TypePack {
            span,
            types: self.punctuated(types),
        }))
    }

    /// `...T` variadic pack element.
    #[must_use]
    pub fn ty_variadic(&self, element: Type) -> Type {
        Type::Variadic(Box::new(VariadicType {
            span: self.next_span(),
            type_value: element,
        }))
    }

    /// `T...` generic pack reference.
    #[must_use]
    pub fn ty_generic_pack(&self, name: &str) -> Type {
        Type::GenericPack(Box::new(GenericPackType {
            span: self.next_span(),
            name: self.ident(name),
        }))
    }

    /// A generic parameter list at a declaration site: `<T, U...>`. Each pair is
    /// a name and whether it is a pack (`T...`); defaults are not synthesized.
    #[must_use]
    pub fn generic_type_list(&self, params: Vec<(&str, bool)>) -> GenericTypeList {
        let span = self.next_span();
        let built: Vec<GenericTypeParam> = params
            .into_iter()
            .map(|(name, is_pack)| GenericTypeParam {
                span: self.next_span(),
                name: self.ident(name),
                is_pack,
                default: None,
            })
            .collect();
        GenericTypeList {
            span,
            params: self.punctuated(built),
        }
    }

    /// Luau alias `[export] type Name [<generics>] = T`.
    #[must_use]
    pub fn type_declaration(
        &self,
        export: bool,
        name: &str,
        generics: Option<GenericTypeList>,
        value: Type,
    ) -> Statement {
        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span: self.next_span(),
            is_exported: export,
            name: self.ident(name),
            generics: generics.map(Box::new),
            type_value: TypeDeclarationValue::Alias(value),
        }))
    }

    /// Luau `[export] type function Name(params) body end` - a compile-time
    /// type function whose body is ordinary Luau.
    #[must_use]
    pub fn type_function(&self, export: bool, name: &str, sig: FnSig, body: Block) -> Statement {
        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span: self.next_span(),
            is_exported: export,
            name: self.ident(name),
            generics: None,
            type_value: TypeDeclarationValue::TypeFunction(Box::new(self.function_body(sig, body))),
        }))
    }

    /// A comment printed on its own line before `stmt`.
    #[must_use]
    pub fn leading_comment(&self, stmt: &Statement, text: &str) -> SyntheticComment {
        SyntheticComment {
            attached_to: stmt.span().start,
            text: CompactString::from(text),
            is_leading: true,
        }
    }

    /// A comment printed as a suffix on `stmt`'s line.
    #[must_use]
    pub fn trailing_comment(&self, stmt: &Statement, text: &str) -> SyntheticComment {
        SyntheticComment {
            attached_to: stmt.span().start,
            text: CompactString::from(text),
            is_leading: false,
        }
    }

    /// A comment printed inside `block` when it is an empty function body
    /// (e.g. `-- unreachable`). Anchored on the block itself since there is
    /// no statement to attach to.
    #[must_use]
    pub fn dangling_comment(&self, block: &Block, text: &str) -> SyntheticComment {
        SyntheticComment {
            attached_to: block.span.start,
            text: CompactString::from(text),
            is_leading: true,
        }
    }

    fn paren_args(&self, args: Vec<Expression>) -> FunctionArgs {
        FunctionArgs::Parenthesized {
            span: self.next_span(),
            args: self.punctuated(args),
        }
    }

    fn punctuated<T>(&self, items: Vec<T>) -> Punctuated<T> {
        Punctuated::from_items(items)
    }
}

/// Double-quoted raw form of UTF-8 `content`. Multibyte characters pass
/// through intact; specials and control characters are escaped.
fn quote_utf8(content: &str) -> CompactString {
    let mut raw = String::with_capacity(content.len() + 2);
    raw.push('"');
    for ch in content.chars() {
        match ch {
            '\\' => raw.push_str("\\\\"),
            '"' => raw.push_str("\\\""),
            '\n' => raw.push_str("\\n"),
            '\r' => raw.push_str("\\r"),
            '\t' => raw.push_str("\\t"),
            '\0'..='\x1f' | '\x7f' => push_decimal_escape(&mut raw, ch as u8),
            _ => raw.push(ch),
        }
    }
    raw.push('"');
    CompactString::from(raw)
}

/// Double-quoted raw form of arbitrary bytes. Everything outside printable
/// ASCII is escaped, so the result is valid UTF-8 regardless of input.
fn quote_bytes(content: &[u8]) -> CompactString {
    let mut raw = String::with_capacity(content.len() + 2);
    raw.push('"');
    for &byte in content {
        match byte {
            b'\\' => raw.push_str("\\\\"),
            b'"' => raw.push_str("\\\""),
            b'\n' => raw.push_str("\\n"),
            b'\r' => raw.push_str("\\r"),
            b'\t' => raw.push_str("\\t"),
            0x20..=0x7e => raw.push(byte as char),
            other => push_decimal_escape(&mut raw, other),
        }
    }
    raw.push('"');
    CompactString::from(raw)
}

/// Escaped interpolated-string segment text: the lexer stores segment text
/// as written in source (escapes intact), so synthesized text must carry the
/// same escapes.
fn interp_text(text: &str) -> CompactString {
    let mut raw = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => raw.push_str("\\\\"),
            '`' => raw.push_str("\\`"),
            '{' => raw.push_str("\\{"),
            '\n' => raw.push_str("\\n"),
            '\r' => raw.push_str("\\r"),
            '\t' => raw.push_str("\\t"),
            '\0'..='\x1f' | '\x7f' => push_decimal_escape(&mut raw, ch as u8),
            _ => raw.push(ch),
        }
    }
    CompactString::from(raw)
}

/// Always three digits: a shorter escape followed by a literal digit
/// (`\57`) would be read as a different byte.
fn push_decimal_escape(raw: &mut String, byte: u8) {
    raw.push('\\');
    raw.push((b'0' + byte / 100) as char);
    raw.push((b'0' + (byte / 10) % 10) as char);
    raw.push((b'0' + byte % 10) as char);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_local_with_binop() {
        let synth = Synth::new();
        let sum = synth.binop(synth.number("1"), BinOp::Add, synth.number("2"));
        let stmt = synth.local(&["x"], vec![sum]);

        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        assert_eq!(local.names.len(), 1);
        let exprs = local.exprs.as_ref().unwrap();
        assert!(matches!(exprs.first().unwrap(), Expression::BinaryOp(_)));
    }

    #[test]
    fn binop_parenthesizes_lower_precedence_operands() {
        let synth = Synth::new();
        // (a + b) * c: the additive LHS must gain parens under `*`.
        let sum = synth.binop(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let product = synth.binop(sum, BinOp::Mul, synth.name_expr("c"));

        let Expression::BinaryOp(product) = &product else {
            panic!("expected binop");
        };
        assert!(
            matches!(product.left, Expression::Parenthesized(_)),
            "lower-precedence LHS not parenthesized"
        );
    }

    #[test]
    fn binop_keeps_higher_precedence_operands_bare() {
        let synth = Synth::new();
        // a * b + c: the multiplicative LHS needs no parens under `+`.
        let product = synth.binop(synth.name_expr("a"), BinOp::Mul, synth.name_expr("b"));
        let sum = synth.binop(product, BinOp::Add, synth.name_expr("c"));

        let Expression::BinaryOp(sum) = &sum else {
            panic!("expected binop");
        };
        assert!(matches!(sum.left, Expression::BinaryOp(_)));
    }

    #[test]
    fn binop_honors_associativity() {
        let synth = Synth::new();
        // Left-nested concat needs parens: (a .. b) .. c is not a .. b .. c.
        let inner = synth.binop(synth.name_expr("a"), BinOp::Concat, synth.name_expr("b"));
        let outer = synth.binop(inner, BinOp::Concat, synth.name_expr("c"));
        let Expression::BinaryOp(outer) = &outer else {
            panic!("expected binop");
        };
        assert!(matches!(outer.left, Expression::Parenthesized(_)));

        // Right-nested subtraction needs parens: a - (b - c).
        let inner = synth.binop(synth.name_expr("b"), BinOp::Sub, synth.name_expr("c"));
        let outer = synth.binop(synth.name_expr("a"), BinOp::Sub, inner);
        let Expression::BinaryOp(outer) = &outer else {
            panic!("expected binop");
        };
        assert!(matches!(outer.right, Expression::Parenthesized(_)));
    }

    #[test]
    fn caret_parenthesizes_unary_lhs() {
        let synth = Synth::new();
        // (-a)^b: without parens this would re-parse as -(a^b).
        let negated = synth.unop(UnOp::Neg, synth.name_expr("a"));
        let power = synth.binop(negated, BinOp::Pow, synth.name_expr("b"));
        let Expression::BinaryOp(power) = &power else {
            panic!("expected binop");
        };
        assert!(matches!(power.left, Expression::Parenthesized(_)));

        // a * -b stays bare: unary is always accepted after a binop.
        let negated = synth.unop(UnOp::Neg, synth.name_expr("b"));
        let product = synth.binop(synth.name_expr("a"), BinOp::Mul, negated);
        let Expression::BinaryOp(product) = &product else {
            panic!("expected binop");
        };
        assert!(matches!(product.right, Expression::UnaryOp(_)));
    }

    #[test]
    fn if_expression_operands_always_parenthesized() {
        let synth = Synth::new();
        let if_expr = synth.if_expr(
            synth.boolean(true),
            synth.number("1"),
            vec![],
            synth.number("2"),
        );
        let sum = synth.binop(synth.name_expr("a"), BinOp::Add, if_expr);
        let Expression::BinaryOp(sum) = &sum else {
            panic!("expected binop");
        };
        assert!(
            matches!(sum.right, Expression::Parenthesized(_)),
            "greedy if-expression must be parenthesized as an operand"
        );
    }

    #[test]
    fn unop_parenthesizes_binop_operand() {
        let synth = Synth::new();
        // -(a + b): without parens this would re-parse as (-a) + b.
        let sum = synth.binop(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let negated = synth.unop(UnOp::Neg, sum);
        let Expression::UnaryOp(negated) = &negated else {
            panic!("expected unop");
        };
        assert!(matches!(negated.operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn type_cast_parenthesizes_compound_operand() {
        let synth = Synth::new();
        let sum = synth.binop(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let cast = synth.type_cast(sum, synth.ty_named("number"));
        let Expression::TypeCast(cast) = &cast else {
            panic!("expected type cast");
        };
        assert!(matches!(cast.expr, Expression::Parenthesized(_)));
    }

    #[test]
    fn prefix_positions_wrap_non_prefix_expressions() {
        let synth = Synth::new();
        // ("s"):rep(2) - a string receiver must be parenthesized.
        let called = synth.method_call(synth.string("s"), "rep", vec![synth.number("2")]);
        let Expression::FunctionCall(call) = &called else {
            panic!("expected call");
        };
        assert!(matches!(call.callee, Expression::Parenthesized(_)));

        // ({}).x - a table prefix must be parenthesized.
        let accessed = synth.field(synth.table(vec![]), "x");
        let Expression::Var(Var::FieldAccess(access)) = &accessed else {
            panic!("expected field access");
        };
        assert!(matches!(access.prefix, Expression::Parenthesized(_)));

        // f().x - a call prefix stays bare.
        let chained = synth.field(synth.call(synth.name_expr("f"), vec![]), "x");
        let Expression::Var(Var::FieldAccess(access)) = &chained else {
            panic!("expected field access");
        };
        assert!(matches!(access.prefix, Expression::FunctionCall(_)));
    }

    #[test]
    fn field_or_index_picks_form_by_name() {
        let synth = Synth::new();
        let table = synth.name_expr("t");
        assert!(matches!(
            synth.field_or_index(table, "valid_name"),
            Expression::Var(Var::FieldAccess(_))
        ));
        for bad in ["not an ident", "1st", "", "end", "goto"] {
            let table = synth.name_expr("t");
            assert!(
                matches!(
                    synth.field_or_index(table, bad),
                    Expression::Var(Var::Index(_))
                ),
                "{bad:?} should use the bracketed form"
            );
        }
    }

    #[test]
    fn number_f64_handles_special_values() {
        let synth = Synth::new();

        let Expression::Number(literal) = synth.number_f64(1.5) else {
            panic!("expected number");
        };
        assert_eq!(literal.text.as_str(), "1.5");

        // Integral floats keep the float subtype on 5.3+.
        let Expression::Number(literal) = synth.number_f64(3.0) else {
            panic!("expected number");
        };
        assert_eq!(literal.text.as_str(), "3.0");

        // Negatives become a unary-minus node (Lua has no negative literals).
        assert!(matches!(synth.number_f64(-2.5), Expression::UnaryOp(_)));
        assert!(matches!(synth.number_f64(-0.0), Expression::UnaryOp(_)));

        // inf -> 1/0, nan -> 0/0, -inf -> -(1/0).
        assert!(matches!(
            synth.number_f64(f64::INFINITY),
            Expression::BinaryOp(_)
        ));
        assert!(matches!(
            synth.number_f64(f64::NAN),
            Expression::BinaryOp(_)
        ));
        let Expression::UnaryOp(neg_inf) = synth.number_f64(f64::NEG_INFINITY) else {
            panic!("expected unop");
        };
        assert!(matches!(neg_inf.operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn number_int_handles_extremes() {
        let synth = Synth::new();
        let Expression::Number(literal) = synth.number_int(42) else {
            panic!("expected number");
        };
        assert_eq!(literal.text.as_str(), "42");

        assert!(matches!(synth.number_int(-7), Expression::UnaryOp(_)));

        // i64::MIN has no decimal literal; the hex form wraps to it.
        let Expression::Number(literal) = synth.number_int(i64::MIN) else {
            panic!("expected number");
        };
        assert_eq!(literal.text.as_str(), "0x8000000000000000");
    }

    #[test]
    fn string_escaping_is_raw() {
        let synth = Synth::new();
        let Expression::StringLiteral(literal) = synth.string("a\"b\n") else {
            panic!("expected string literal");
        };
        assert_eq!(literal.text.as_str(), "\"a\\\"b\\n\"");
    }

    #[test]
    fn string_bytes_escapes_non_utf8() {
        let synth = Synth::new();
        let Expression::StringLiteral(literal) = synth.string_bytes(&[0xff, b'a', 0x01, b'7'])
        else {
            panic!("expected string literal");
        };
        // Escapes are zero-padded to three digits so the following literal
        // digit `7` cannot extend them.
        assert_eq!(literal.text.as_str(), "\"\\255a\\0017\"");
    }

    #[test]
    fn long_string_picks_collision_free_level() {
        let synth = Synth::new();
        let Expression::StringLiteral(literal) = synth.long_string("plain text") else {
            panic!("expected string literal");
        };
        assert_eq!(literal.text.as_str(), "[[plain text]]");

        let Expression::StringLiteral(literal) = synth.long_string("has ]] inside") else {
            panic!("expected string literal");
        };
        assert_eq!(literal.text.as_str(), "[=[has ]] inside]=]");

        // A leading newline would be swallowed; it gets doubled.
        let Expression::StringLiteral(literal) = synth.long_string("\ntext") else {
            panic!("expected string literal");
        };
        assert_eq!(literal.text.as_str(), "[[\n\ntext]]");

        // A trailing `]` at level 0 would merge with the closer.
        let Expression::StringLiteral(literal) = synth.long_string("x]") else {
            panic!("expected string literal");
        };
        assert_eq!(literal.text.as_str(), "[=[x]]=]");
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
        let stmt = synth.local_full(
            vec![synth.attributed_name("x", Some(optional), None)],
            vec![synth.nil()],
        );

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
        let sig = FnSig {
            params: vec![synth.param_typed("n", synth.ty_named("number"))],
            return_type: Some(synth.ty_named("number")),
            ..FnSig::default()
        };
        let ret = synth.return_(vec![synth.name_expr("n")]);
        let func = synth.function_def_full(Vec::new(), sig, synth.block(vec![], Some(ret)));

        let Expression::FunctionDef(def) = &func else {
            panic!("expected function def");
        };
        assert_eq!(def.body.params.len(), 1);
        assert!(def.body.params.first().unwrap().type_annotation.is_some());
        assert!(def.body.return_type.is_some());
    }

    #[test]
    fn builds_generic_function_with_attributes() {
        let synth = Synth::new();
        let sig = FnSig {
            generics: Some(synth.generic_type_list(vec![("T", false)])),
            params: vec![synth.param_typed("value", synth.ty_named("T"))],
            return_type: Some(synth.ty_named("T")),
            ..FnSig::default()
        };
        let stmt = synth.local_function_full(
            "id",
            vec![synth.function_attribute("native", None)],
            sig,
            synth.block(vec![], None),
        );

        let Statement::LocalFunction(func) = &stmt else {
            panic!("expected local function");
        };
        assert_eq!(func.attributes.len(), 1);
        assert!(func.body.generics.is_some());
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

        let Statement::GenericFor(generic) = &stmt else {
            panic!("expected generic for");
        };
        assert_eq!(generic.names.len(), 2);
        assert_eq!(generic.exprs.len(), 1);
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
        let Statement::NumericFor(numeric) = &stmt else {
            panic!("expected numeric for");
        };
        assert!(numeric.type_annotation.is_some());
    }

    #[test]
    fn builds_mixed_table() {
        let synth = Synth::new();
        let table = synth.table(vec![
            SynthField::Positional(synth.number("1")),
            SynthField::Named("a", synth.number("2")),
            SynthField::Bracketed(synth.string("k"), synth.boolean(true)),
        ]);

        let Expression::TableConstructor(constructor) = &table else {
            panic!("expected table constructor");
        };
        assert_eq!(constructor.fields.len(), 3);
        assert!(matches!(
            constructor.fields.items[0],
            Field::Positional { .. }
        ));
        assert!(matches!(constructor.fields.items[1], Field::Named { .. }));
        assert!(matches!(
            constructor.fields.items[2],
            Field::Bracketed { .. }
        ));
    }

    #[test]
    fn array_and_record_sugar() {
        let synth = Synth::new();
        let Expression::TableConstructor(array) =
            synth.array(vec![synth.number("1"), synth.number("2")])
        else {
            panic!("expected table");
        };
        assert!(
            array
                .fields
                .iter()
                .all(|field| matches!(field, Field::Positional { .. }))
        );

        // Unsafe keys fall back to the bracketed form.
        let Expression::TableConstructor(record) = synth.record(vec![
            ("good", synth.number("1")),
            ("not valid", synth.number("2")),
        ]) else {
            panic!("expected table");
        };
        assert!(matches!(record.fields.items[0], Field::Named { .. }));
        assert!(matches!(record.fields.items[1], Field::Bracketed { .. }));
    }

    #[test]
    fn builds_sugar_call_forms() {
        let synth = Synth::new();
        let Expression::FunctionCall(call) =
            synth.call_string(synth.name_expr("require"), "module")
        else {
            panic!("expected call");
        };
        assert!(matches!(call.args, FunctionArgs::StringLiteral(_)));

        let Expression::FunctionCall(call) = synth.call_table(
            synth.name_expr("configure"),
            vec![SynthField::Named("debug", synth.boolean(true))],
        ) else {
            panic!("expected call");
        };
        assert!(matches!(call.args, FunctionArgs::TableConstructor(_)));

        let Expression::FunctionCall(call) =
            synth.method_call_string(synth.name_expr("s"), "gsub", "pattern")
        else {
            panic!("expected call");
        };
        assert!(call.method.is_some());
        assert!(matches!(call.args, FunctionArgs::StringLiteral(_)));
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
    fn starting_at_offsets_spans() {
        let synth = Synth::starting_at(1000);
        assert_eq!(synth.nil().span().start, 1000);
    }

    #[test]
    fn builds_variadic_function() {
        let synth = Synth::new();
        let sig = FnSig {
            params: vec![synth.param("first")],
            vararg: Some(synth.vararg_param(None)),
            ..FnSig::default()
        };
        let func = synth.function_def_full(Vec::new(), sig, synth.block(vec![], None));

        let Expression::FunctionDef(def) = &func else {
            panic!("expected function def");
        };
        assert_eq!(def.body.params.len(), 1);
        let vararg = def.body.vararg.as_ref().expect("vararg present");
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
        let decl = synth.global_decl(
            vec![synth.attributed_name("shared", None, Some("const"))],
            vec![],
        );
        let Statement::GlobalDeclaration(decl) = &decl else {
            panic!("expected global declaration");
        };
        assert!(decl.names.first().unwrap().attrib.is_some());

        let func = synth.global_function("main", FnSig::default(), synth.block(vec![], None));
        assert!(matches!(func, Statement::GlobalFunction(_)));

        let Statement::GlobalStar(star) = synth.global_star(Some("const")) else {
            panic!("expected global star");
        };
        assert!(star.attrib.is_some());
    }

    #[test]
    fn builds_compound_assignment() {
        let synth = Synth::new();
        let stmt = synth.compound_assign(
            synth.name_var("counter"),
            CompoundOp::AddAssign,
            synth.number("1"),
        );

        let Statement::CompoundAssignment(compound) = &stmt else {
            panic!("expected compound assignment");
        };
        assert!(matches!(compound.var, Var::Name(_)));
        assert_eq!(compound.op, CompoundOp::AddAssign);
    }

    #[test]
    fn builds_interpolated_string_with_escapes() {
        let synth = Synth::new();
        let inner = synth.name_expr("value");
        // `a{value}b` -> InterpBegin("a") + expr, then InterpEnd("b") terminator.
        let string = synth.interpolated_string(vec![
            SynthInterpPart::Text("a"),
            SynthInterpPart::Expr(inner),
            SynthInterpPart::Text("b{`"),
        ]);

        let Expression::InterpolatedString(interp) = &string else {
            panic!("expected interpolated string");
        };
        assert_eq!(interp.segments.len(), 2);
        assert!(matches!(
            interp.segments[0].literal.kind,
            TokenKind::InterpBegin(_)
        ));
        assert!(interp.segments[0].expr.is_some());
        let TokenKind::InterpEnd(text) = &interp.segments[1].literal.kind else {
            panic!("expected InterpEnd");
        };
        // `{` and backtick carry their escapes, as the lexer stores them.
        assert_eq!(text.as_str(), "b\\{\\`");
        assert!(interp.segments[1].expr.is_none());
    }

    #[test]
    fn interpolated_string_derives_segment_shape() {
        let synth = Synth::new();

        // Plain text keeps the lexer's Begin("") + End(text) pair; adjacent
        // text parts merge.
        let plain =
            synth.interpolated_string(vec![SynthInterpPart::Text("a"), SynthInterpPart::Text("b")]);
        let Expression::InterpolatedString(interp) = &plain else {
            panic!("expected interpolated string");
        };
        assert_eq!(interp.segments.len(), 2);
        let TokenKind::InterpBegin(begin) = &interp.segments[0].literal.kind else {
            panic!("expected InterpBegin");
        };
        assert_eq!(begin.as_str(), "");
        let TokenKind::InterpEnd(end) = &interp.segments[1].literal.kind else {
            panic!("expected InterpEnd");
        };
        assert_eq!(end.as_str(), "ab");

        // Two expressions produce a Mid segment between Begin and End.
        let multi = synth.interpolated_string(vec![
            SynthInterpPart::Expr(synth.name_expr("x")),
            SynthInterpPart::Text("-"),
            SynthInterpPart::Expr(synth.name_expr("y")),
        ]);
        let Expression::InterpolatedString(interp) = &multi else {
            panic!("expected interpolated string");
        };
        assert_eq!(interp.segments.len(), 3);
        assert!(matches!(
            interp.segments[1].literal.kind,
            TokenKind::InterpMid(_)
        ));
        assert!(interp.segments[2].expr.is_none());
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
        let generics = synth.generic_type_list(vec![("T", false), ("Rest", true)]);
        let stmt = synth.type_declaration(true, "Alias", Some(generics), synth.ty_named("T"));

        let Statement::TypeDeclaration(decl) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(decl.is_exported);
        assert!(matches!(decl.type_value, TypeDeclarationValue::Alias(_)));
        let generics = decl.generics.as_ref().expect("generics present");
        assert_eq!(generics.params.len(), 2);
        assert!(generics.params.last().unwrap().is_pack);
    }

    #[test]
    fn builds_type_function() {
        let synth = Synth::new();
        let sig = FnSig {
            params: vec![synth.param("ty")],
            ..FnSig::default()
        };
        let stmt = synth.type_function(false, "Partial", sig, synth.block(vec![], None));
        let Statement::TypeDeclaration(decl) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(!decl.is_exported);
        assert!(matches!(
            decl.type_value,
            TypeDeclarationValue::TypeFunction(_)
        ));
    }

    #[test]
    fn builds_full_table_type() {
        let synth = Synth::new();
        let ty = synth.ty_table(vec![
            SynthTypeField::Named {
                access: Some(TypeFieldAccess::Read),
                name: "id",
                value: synth.ty_named("number"),
            },
            SynthTypeField::Indexer {
                access: None,
                key: synth.ty_named("string"),
                value: synth.ty_named("boolean"),
            },
        ]);
        let Type::Table(table) = &ty else {
            panic!("expected table type");
        };
        assert!(matches!(
            &table.fields.items[0],
            TypeField::Named {
                access: Some(_),
                ..
            }
        ));
        assert!(matches!(&table.fields.items[1], TypeField::Indexer { .. }));
    }

    #[test]
    fn builds_remaining_type_forms() {
        let synth = Synth::new();
        assert!(matches!(
            synth.ty_typeof(synth.name_expr("x")),
            Type::Typeof(_)
        ));
        assert!(matches!(
            synth.ty_paren(synth.ty_named("T")),
            Type::Parenthesized(_)
        ));
        assert!(matches!(synth.ty_generic_pack("T"), Type::GenericPack(_)));
        assert!(matches!(
            synth.ty_singleton_bool(true),
            Type::Singleton(Token {
                kind: TokenKind::True,
                ..
            })
        ));
        assert!(matches!(
            synth.ty_singleton_nil(),
            Type::Singleton(Token {
                kind: TokenKind::Nil,
                ..
            })
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
        let stmt = synth.function_decl(
            &["a", "b"],
            Some("greet"),
            &["self"],
            synth.block(vec![], None),
        );

        let Statement::FunctionDecl(decl) = &stmt else {
            panic!("expected function declaration");
        };
        assert_eq!(decl.name.names.len(), 2);
        assert!(decl.name.method.is_some());
    }

    #[test]
    fn comment_helpers_anchor_on_statements() {
        let synth = Synth::new();
        let stmt = synth.local(&["x"], vec![synth.number("1")]);
        let leading = synth.leading_comment(&stmt, "before");
        let trailing = synth.trailing_comment(&stmt, "after");
        assert_eq!(leading.attached_to, stmt.span().start);
        assert!(leading.is_leading);
        assert_eq!(trailing.attached_to, stmt.span().start);
        assert!(!trailing.is_leading);

        let block = synth.block(vec![], None);
        let dangling = synth.dangling_comment(&block, "unreachable");
        assert_eq!(dangling.attached_to, block.span.start);
    }

    #[test]
    fn identifier_validity() {
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("_private"));
        assert!(is_valid_identifier("snake_case2"));
        // Context-sensitive words are identifiers.
        assert!(is_valid_identifier("type"));
        assert!(is_valid_identifier("continue"));
        assert!(is_valid_identifier("goto"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("1st"));
        assert!(!is_valid_identifier("has space"));
        assert!(!is_valid_identifier("end"));
        assert!(!is_valid_identifier("Ünicode"));
    }

    #[test]
    fn identifier_validity_per_version() {
        assert!(is_valid_identifier_in("goto", LuaVersion::Lua51));
        assert!(is_valid_identifier_in("goto", LuaVersion::Luau));
        assert!(!is_valid_identifier_in("goto", LuaVersion::Lua54));
        assert!(is_valid_identifier_in("global", LuaVersion::Lua54));
        assert!(!is_valid_identifier_in("global", LuaVersion::Lua55));
        assert!(!is_valid_identifier_in("end", LuaVersion::Luau));
        assert!(!is_valid_identifier_in("", LuaVersion::Luau));
    }

    #[test]
    fn version_pinned_field_names() {
        // Unpinned: `goto` is a keyword somewhere, so it gets bracketed.
        let synth = Synth::new();
        assert!(matches!(
            synth.field_or_index(synth.name_expr("t"), "goto"),
            Expression::Var(Var::Index(_))
        ));

        // Pinned to Luau: `goto` is a plain identifier there.
        let synth = Synth::new().with_version(LuaVersion::Luau);
        assert!(matches!(
            synth.field_or_index(synth.name_expr("t"), "goto"),
            Expression::Var(Var::FieldAccess(_))
        ));

        // Pinned to 5.4: still a keyword, still bracketed.
        let synth = Synth::new().with_version(LuaVersion::Lua54);
        assert!(matches!(
            synth.field_or_index(synth.name_expr("t"), "goto"),
            Expression::Var(Var::Index(_))
        ));
    }

    #[test]
    fn cast_operand_parenthesized_before_type_extending_ops() {
        let synth = Synth::new();
        // (a :: T) < b: bare, the type parser reads `<` as generic args.
        for op in [BinOp::Lt, BinOp::BitAnd, BinOp::BitOr] {
            let cast = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
            let compared = synth.binop(cast, op, synth.name_expr("b"));
            let Expression::BinaryOp(compared) = &compared else {
                panic!("expected binop");
            };
            assert!(
                matches!(compared.left, Expression::Parenthesized(_)),
                "cast before {op:?} must be parenthesized"
            );
        }

        // a + b :: T stays bare: `+` cannot extend a printed type.
        let cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let sum = synth.binop(synth.name_expr("a"), BinOp::Add, cast);
        let Expression::BinaryOp(sum) = &sum else {
            panic!("expected binop");
        };
        assert!(matches!(sum.right, Expression::TypeCast(_)));

        // The cast may sit at the end of a larger left operand: the whole
        // operand gains parens, `(a + b :: T) < c`.
        let cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let sum = synth.binop(synth.name_expr("a"), BinOp::Add, cast);
        let compared = synth.binop(sum, BinOp::Lt, synth.name_expr("c"));
        let Expression::BinaryOp(compared) = &compared else {
            panic!("expected binop");
        };
        assert!(matches!(compared.left, Expression::Parenthesized(_)));

        // A cast on the right of `<` ends the statement; no parens needed.
        let cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let compared = synth.binop(synth.name_expr("a"), BinOp::Lt, cast);
        let Expression::BinaryOp(compared) = &compared else {
            panic!("expected binop");
        };
        assert!(matches!(compared.right, Expression::TypeCast(_)));
    }

    #[test]
    fn chained_cast_parenthesized() {
        let synth = Synth::new();
        // (a :: T) :: U: `a :: T :: U` is a parse error in Luau.
        let inner = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
        let outer = synth.type_cast(inner, synth.ty_named("U"));
        let Expression::TypeCast(outer) = &outer else {
            panic!("expected type cast");
        };
        assert!(matches!(outer.expr, Expression::Parenthesized(_)));
    }

    #[test]
    fn single_value_wraps_multi_value_forms() {
        let synth = Synth::new();
        let call = synth.call(synth.name_expr("f"), vec![]);
        assert!(matches!(
            synth.single_value(call),
            Expression::Parenthesized(_)
        ));
        assert!(matches!(
            synth.single_value(synth.vararg()),
            Expression::Parenthesized(_)
        ));
        // Single-value expressions pass through untouched.
        assert!(matches!(
            synth.single_value(synth.name_expr("x")),
            Expression::Var(_)
        ));
        assert!(matches!(
            synth.single_value(synth.number_int(1)),
            Expression::Number(_)
        ));
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
        let Expression::Number(literal) = synth.number_f64(1e300) else {
            panic!("expected number");
        };
        assert_eq!(literal.text.as_str(), "1e300");

        let Expression::Number(literal) = synth.number_f64(100.0) else {
            panic!("expected number");
        };
        assert_eq!(literal.text.as_str(), "1e2");

        // Short plain forms win ties and stay readable.
        let Expression::Number(literal) = synth.number_f64(3.0) else {
            panic!("expected number");
        };
        assert_eq!(literal.text.as_str(), "3.0");
        let Expression::Number(literal) = synth.number_f64(1.5) else {
            panic!("expected number");
        };
        assert_eq!(literal.text.as_str(), "1.5");
    }

    #[test]
    fn builds_const_declarations() {
        let synth = Synth::new();
        let stmt = synth.const_local(
            vec![synth.attributed_name("frozen", None, None)],
            vec![synth.number_int(1)],
        );
        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        assert!(local.is_const);

        let plain = synth.local(&["thawed"], vec![synth.number_int(2)]);
        let Statement::LocalAssignment(local) = &plain else {
            panic!("expected local assignment");
        };
        assert!(!local.is_const);

        let func = synth.const_function(
            "pinned",
            Vec::new(),
            FnSig::default(),
            synth.block(vec![], None),
        );
        let Statement::LocalFunction(func) = &func else {
            panic!("expected local function");
        };
        assert!(func.is_const);
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
}
