//! Leaf text comes from token and literal values, never from source, so
//! synthetic ASTs stay printable.

use luck_ast::{
    BinaryOp, Expression, FieldAccess, IfExpression, InterpolatedString, ParenthesizedExpression,
    UnaryOp, Var, ensure_sufficient_stack, expr_starts_with_brace,
};
use luck_token::{BinOp, CompactString, Token, TokenKind, UnOp};

use crate::format_type::ExplicitTypeArgs;
use crate::ir::{
    Format, Formatter, format_with, group, indent, soft_line, soft_line_or_space, space, token,
};
use crate::numbers::normalize_number;
use crate::quotes::normalize_quote;

impl Format for Expression {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Nil(_) => token("nil").fmt(f),
            Self::False(_) => token("false").fmt(f),
            Self::True(_) => token("true").fmt(f),
            Self::VarArg(_) => token("...").fmt(f),

            Self::Number(literal) | Self::Integer(literal) => {
                let normalized = normalize_number(&literal.text, f.options().hexadecimal_case);
                f.write_text(normalized);
            }
            Self::StringLiteral(literal) => {
                let normalized = normalize_quote(&literal.text, f.options().quote_style);
                f.write_text(normalized);
            }

            Self::Var(var) => var.fmt(f),
            Self::BinaryOp(binary_op) => binary_op.fmt(f),
            Self::UnaryOp(unary_op) => unary_op.fmt(f),
            Self::Parenthesized(parenthesized) => parenthesized.fmt(f),
            Self::TableConstructor(table) => table.fmt(f),

            Self::FunctionCall(call) => call.fmt(f),
            Self::FunctionDef(function_def) => {
                // Luau attributes stay on the same line as `function` here.
                for attribute in &function_def.attributes {
                    (attribute, space()).fmt(f);
                }
                (token("function"), &function_def.body).fmt(f);
            }

            Self::IfExpression(if_expr) => if_expr.fmt(f), // Luau
            Self::InterpolatedString(interpolated) => interpolated.fmt(f), // Luau
            // Luau
            Self::TypeCast(cast) => ensure_sufficient_stack(|| {
                (&cast.expr, space(), token("::"), space(), &cast.type_annotation).fmt(f);
            }),
            // Luau
            Self::TypeInstantiation(instantiation) => ensure_sufficient_stack(|| {
                (&instantiation.expr, ExplicitTypeArgs(&instantiation.type_args)).fmt(f);
            }),

            // A parse-recovery placeholder has no text to print.
            Self::Error(_) => {}
        }
    }
}

impl Format for Var {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Name(name) => name.fmt(f),
            Self::FieldAccess(access) => access.fmt(f),
            Self::Index(index) => ensure_sufficient_stack(|| {
                (&index.prefix, BracketedKey(&index.index)).fmt(f);
            }),
        }
    }
}

pub(crate) struct BracketedKey<'expr>(pub &'expr Expression);

impl Format for BracketedKey<'_> {
    fn fmt(&self, f: &mut Formatter) {
        // Right after the opening `[`, a long-bracket string would lex as
        // `[[`, a long-string opener, so it's padded with spaces.
        if starts_with_bracket(self.0) {
            (token("["), space(), self.0, space(), token("]")).fmt(f);
        } else {
            (token("["), self.0, token("]")).fmt(f);
        }
    }
}

fn starts_with_bracket(expr: &Expression) -> bool {
    let mut current = expr;
    loop {
        current = match current {
            Expression::StringLiteral(literal) => return literal.text.starts_with('['),
            Expression::BinaryOp(binary_op) => &binary_op.lhs,
            // Luau
            Expression::TypeCast(cast) => &cast.expr,
            // Luau
            Expression::TypeInstantiation(instantiation) => &instantiation.expr,
            Expression::FunctionCall(call) => &call.callee,
            Expression::Var(Var::Index(index)) => &index.prefix,
            Expression::Var(Var::FieldAccess(access)) => &access.prefix,
            Expression::Var(Var::Name(_))
            | Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_) => return false,
        };
    }
}

fn collect_access_chain(access: &FieldAccess) -> (&Expression, Vec<&Token>) {
    let mut names = vec![&access.name];
    let mut current = &access.prefix;
    while let Expression::Var(var) = current {
        match var {
            Var::FieldAccess(inner) => {
                names.push(&inner.name);
                current = &inner.prefix;
            }
            Var::Name(_) | Var::Index(_) => break,
        }
    }
    names.reverse();
    (current, names)
}

/// A dot chain of three or more accesses gets its own group so it can break
/// one segment per line; shorter chains stay glued to their prefix.
///
/// The root stays outside the group, so a break inside it, as in a callback
/// it's passed, doesn't break the chain after it.
impl Format for FieldAccess {
    fn fmt(&self, f: &mut Formatter) {
        let (root, names) = collect_access_chain(self);
        ensure_sufficient_stack(|| {
            if names.len() >= 3 {
                (
                    root,
                    group(indent(format_with(|indented| {
                        for name in &names {
                            (soft_line(), token("."), *name).fmt(indented);
                        }
                    }))),
                )
                    .fmt(f);
            } else {
                (&self.prefix, token("."), &self.name).fmt(f);
            }
        });
    }
}

/// A chain is a run of operators at one precedence level. An operand that
/// binds tighter keeps a group of its own, so `a == b or c == d` breaks at
/// `or` alone.
///
/// Runs of the right-associative `..` or `^` nest on the right.
///
/// Left nested, each level would be its own group and re-run `fits` over its remainder, which is
/// quadratic on long concat chains. Splicing them in keeps the emitted text the same and lays the
/// run out like a left-associative chain.
fn collect_binary_chain(binary_op: &BinaryOp) -> (&Expression, Vec<(BinOp, &Expression)>) {
    let mut parts = Vec::new();
    let mut current = binary_op;
    loop {
        parts.push((current.op, &current.rhs));
        let Expression::BinaryOp(left_binary_op) = &current.lhs else {
            break;
        };
        if left_binary_op.op.precedence().0 != current.op.precedence().0 {
            break;
        }
        current = left_binary_op;
    }
    parts.reverse();

    let mut flattened: Vec<(BinOp, &Expression)> = Vec::with_capacity(parts.len());
    for (mut op, mut right) in parts {
        while let Expression::BinaryOp(inner) = right {
            let is_same_right_associative_op =
                matches!((op, inner.op), (BinOp::Concat, BinOp::Concat) | (BinOp::Pow, BinOp::Pow));
            if !is_same_right_associative_op {
                break;
            }
            flattened.push((op, &inner.lhs));
            op = inner.op;
            right = &inner.rhs;
        }
        flattened.push((op, right));
    }

    (&current.lhs, flattened)
}

impl Format for BinaryOp {
    fn fmt(&self, f: &mut Formatter) {
        write_binary_chain(f, self, true);
    }
}

/// A chain that breaks with its operators level with its first operand,
/// for a condition that already sits on an indented line of its own.
pub(crate) struct LevelChain<'chain>(pub &'chain BinaryOp);

impl Format for LevelChain<'_> {
    fn fmt(&self, f: &mut Formatter) {
        write_binary_chain(f, self.0, false);
    }
}

fn write_binary_chain(f: &mut Formatter, binary_op: &BinaryOp, is_indented: bool) {
    let (first, chain) = collect_binary_chain(binary_op);
    let operators = format_with(|lines| {
        for (op, right_expr) in &chain {
            (soft_line_or_space(), token(op.static_text()), space(), *right_expr).fmt(lines);
        }
    });
    // One group over the whole chain, so it's either flat or breaks before
    // every operator.
    ensure_sufficient_stack(|| {
        if is_indented {
            group((first, indent(&operators))).fmt(f);
        } else {
            group((first, &operators)).fmt(f);
        }
    });
}

/// `-` right before a nested unary `-` would form `--` and start a comment, so
/// the pair needs a space (`- -x`).
fn is_double_minus_hazard(op: UnOp, operand: &Expression) -> bool {
    op == UnOp::Neg && matches!(operand, Expression::UnaryOp(inner) if inner.op == UnOp::Neg)
}

impl Format for UnaryOp {
    fn fmt(&self, f: &mut Formatter) {
        token(self.op.static_text()).fmt(f);
        // `not` is a keyword operator: always separated from its operand.
        if self.op == UnOp::Not || is_double_minus_hazard(self.op, &self.operand) {
            space().fmt(f);
        }
        ensure_sufficient_stack(|| self.operand.fmt(f));
    }
}

impl Format for ParenthesizedExpression {
    fn fmt(&self, f: &mut Formatter) {
        ensure_sufficient_stack(|| (token("("), &self.expr, token(")")).fmt(f));
    }
}

impl Format for IfExpression {
    fn fmt(&self, f: &mut Formatter) {
        // Luau: one group, so a long if-expression starts an indented line at
        // each `then`, `elseif`, and `else`.
        group((
            token("if"),
            space(),
            &self.condition,
            indent(format_with(|branches| {
                (soft_line_or_space(), token("then"), space(), &self.then_expr).fmt(branches);

                for clause in &self.elseif_clauses {
                    (
                        soft_line_or_space(),
                        token("elseif"),
                        space(),
                        &clause.condition,
                        space(),
                        token("then"),
                        space(),
                        &clause.expr,
                    )
                        .fmt(branches);
                }

                (soft_line_or_space(), token("else"), space(), &self.else_expr).fmt(branches);
            })),
        ))
        .fmt(f);
    }
}

impl Format for InterpolatedString {
    fn fmt(&self, f: &mut Formatter) {
        // Luau. Punctuation follows expression presence, not token kind: a
        // plain `` `text` `` parses as `InterpolatedStringBegin("")` plus
        // `InterpolatedStringEnd(text)` with no expression, and must print
        // without braces.
        let mut opener = Some('`');
        for segment in &self.segments {
            let part = if let TokenKind::InterpolatedStringBegin(text)
            | TokenKind::InterpolatedStringMiddle(text)
            | TokenKind::InterpolatedStringEnd(text) = &segment.literal.kind
            {
                text.as_str()
            } else {
                ""
            };
            let mut piece = CompactString::default();
            piece.extend(opener);
            piece.push_str(part);
            if segment.expr.is_some() {
                piece.push('{');
            }
            f.write_text(piece);
            if let Some(expr) = &segment.expr {
                // Luau rejects `{{`, so an expression starting with `{`
                // gets a space after the opening brace, and one before the
                // closing brace to match.
                if expr_starts_with_brace(expr) {
                    (space(), expr, space()).fmt(f);
                } else {
                    expr.fmt(f);
                }
            }
            opener = segment.expr.is_some().then_some('}');
        }
        f.write_text("`");
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::{parse_expr, render};
    use crate::{FormatOptions, IndentStyle, QuoteStyle};

    fn print_expr(source: &str, quote_style: QuoteStyle, line_width: u16) -> String {
        let options = FormatOptions {
            line_width,
            indent_style: IndentStyle::Spaces,
            quote_style,
            ..FormatOptions::default()
        };
        render(&parse_expr(source), options)
    }

    #[test]
    fn string_requoted_to_double() {
        assert_eq!(print_expr("'hi'", QuoteStyle::Double, 80), "\"hi\"");
    }

    #[test]
    fn long_bracket_string_untouched() {
        assert_eq!(print_expr("[[raw]]", QuoteStyle::Double, 80), "[[raw]]");
    }

    #[test]
    fn binary_chain_flat_fits() {
        assert_eq!(print_expr("1 + 2 + 3", QuoteStyle::Double, 80), "1 + 2 + 3");
    }

    #[test]
    fn binary_chain_breaks_before_op() {
        let output = print_expr("aaaa + bbbb + cccc", QuoteStyle::Double, 10);
        assert!(output.starts_with("aaaa"));
        assert!(output.contains('\n'));
        assert!(output.contains("+ bbbb"));
    }

    #[test]
    fn unary_not_keeps_keyword_space() {
        assert_eq!(print_expr("not x", QuoteStyle::Double, 80), "not x");
    }

    #[test]
    fn unary_minus_no_space() {
        assert_eq!(print_expr("-x", QuoteStyle::Double, 80), "-x");
    }

    #[test]
    fn nested_unary_minus_gets_space() {
        // Must not collapse to `--x` (a comment).
        assert_eq!(print_expr("- -x", QuoteStyle::Double, 80), "- -x");
    }

    #[test]
    fn short_field_access_stays_glued() {
        assert_eq!(print_expr("a.b.c", QuoteStyle::Double, 80), "a.b.c");
    }

    #[test]
    fn long_field_chain_breaks_per_segment() {
        let output = print_expr("aaaa.bbbb.cccc.dddd", QuoteStyle::Double, 10);
        assert!(output.starts_with("aaaa"));
        assert!(output.contains(".bbbb"));
        assert!(output.contains('\n'));
    }

    #[test]
    fn parenthesized_preserved() {
        assert_eq!(print_expr("(1 + 2)", QuoteStyle::Double, 80), "(1 + 2)");
    }

    #[test]
    fn if_expr_flat() {
        assert_eq!(print_expr("if a then b else c", QuoteStyle::Double, 80), "if a then b else c");
    }

    #[test]
    fn interpolated_string_roundtrips() {
        assert_eq!(print_expr("`hi {x} there`", QuoteStyle::Double, 80), "`hi {x} there`");
    }

    #[test]
    fn plain_interpolated_string_roundtrips() {
        // A plain backtick string parses as a Begin("") + End(text) pair with
        // no expression; it must not gain a spurious `{}` hole.
        assert_eq!(print_expr("`plain text`", QuoteStyle::Double, 80), "`plain text`");
    }
}
