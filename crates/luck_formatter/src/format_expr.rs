//! Expression layout.
//!
//! Leaf text comes from token and literal values, never from source, so
//! synthetic ASTs stay printable.

use luck_ast::{
    BinaryOp, Expression, FieldAccess, IfExpression, InterpolatedString, ParenthesizedExpression,
    UnaryOp, Var, expr_starts_with_brace,
};
use luck_token::{BinOp, CompactString, Token, TokenKind, UnOp};

use crate::format_type::ExplicitTypeArgs;
use crate::ir::{
    Format, Formatter, format_with, group, indent, soft_line, soft_line_or_space, space, token,
};
use crate::numbers::normalize_number;
use crate::quotes::normalize_quote;

impl Format for Expression {
    fn fmt(&self, formatter: &mut Formatter) {
        match self {
            Self::Nil(_) => token("nil").fmt(formatter),
            Self::False(_) => token("false").fmt(formatter),
            Self::True(_) => token("true").fmt(formatter),
            Self::VarArg(_) => token("...").fmt(formatter),

            Self::Number(literal) | Self::Integer(literal) => {
                let normalized =
                    normalize_number(&literal.text, formatter.options().hexadecimal_case);
                formatter.write_text(normalized);
            }
            Self::StringLiteral(literal) => {
                let normalized = normalize_quote(&literal.text, formatter.options().quote_style);
                formatter.write_text(normalized);
            }

            Self::Var(var) => var.fmt(formatter),
            Self::BinaryOp(binary_op) => binary_op.fmt(formatter),
            Self::UnaryOp(unary_op) => unary_op.fmt(formatter),
            Self::Parenthesized(parenthesized) => parenthesized.fmt(formatter),
            Self::TableConstructor(table) => table.fmt(formatter),

            Self::FunctionCall(call) => call.fmt(formatter),
            Self::FunctionDef(function_def) => {
                // Luau attributes stay on the same line as `function` here.
                for attribute in &function_def.attributes {
                    (attribute, space()).fmt(formatter);
                }
                (token("function"), &function_def.body).fmt(formatter);
            }

            Self::IfExpression(if_expr) => if_expr.fmt(formatter), // Luau
            Self::InterpolatedString(interpolated) => interpolated.fmt(formatter), // Luau
            // Luau
            Self::TypeCast(cast) => {
                (&cast.expr, space(), token("::"), space(), &cast.type_annotation).fmt(formatter);
            }
            // Luau
            Self::TypeInstantiation(instantiation) => {
                (&instantiation.expr, ExplicitTypeArgs(&instantiation.type_args)).fmt(formatter);
            }

            // A parse-recovery placeholder has no text to print.
            Self::Error(_) => {}
        }
    }
}

impl Format for Var {
    fn fmt(&self, formatter: &mut Formatter) {
        match self {
            Self::Name(name) => name.fmt(formatter),
            Self::FieldAccess(access) => access.fmt(formatter),
            Self::Index(index) => (&index.prefix, BracketedKey(&index.index)).fmt(formatter),
        }
    }
}

/// `[key]`, as an index or a table field's key.
pub(crate) struct BracketedKey<'expr>(pub &'expr Expression);

impl Format for BracketedKey<'_> {
    fn fmt(&self, formatter: &mut Formatter) {
        // Right after the opening `[`, a long-bracket string would lex as
        // `[[`, a long-string opener, so it's padded with spaces.
        if starts_with_bracket(self.0) {
            (token("["), space(), self.0, space(), token("]")).fmt(formatter);
        } else {
            (token("["), self.0, token("]")).fmt(formatter);
        }
    }
}

/// Whether the expression's leftmost token is a long-bracket string.
fn starts_with_bracket(expr: &Expression) -> bool {
    match expr {
        Expression::StringLiteral(literal) => literal.text.starts_with('['),
        Expression::BinaryOp(binary_op) => starts_with_bracket(&binary_op.left),
        // Luau
        Expression::TypeCast(cast) => starts_with_bracket(&cast.expr),
        // Luau
        Expression::TypeInstantiation(instantiation) => starts_with_bracket(&instantiation.expr),
        Expression::FunctionCall(call) => starts_with_bracket(&call.callee),
        Expression::Var(var) => match var {
            Var::Name(_) => false,
            Var::Index(index) => starts_with_bracket(&index.prefix),
            Var::FieldAccess(access) => starts_with_bracket(&access.prefix),
        },
        Expression::Nil(_)
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
        | Expression::Error(_) => false,
    }
}

/// Collect a left-recursive dot chain `a.b.c.d` into its non-`.` root and the
/// field-name tokens in source order.
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
impl Format for FieldAccess {
    fn fmt(&self, formatter: &mut Formatter) {
        let (root, names) = collect_access_chain(self);
        if names.len() >= 3 {
            group((
                root,
                indent(format_with(|indented| {
                    for name in &names {
                        (soft_line(), token("."), *name).fmt(indented);
                    }
                })),
            ))
            .fmt(formatter);
        } else {
            (&self.prefix, token("."), &self.name).fmt(formatter);
        }
    }
}

/// Flattens a binary chain into its leftmost operand and the
/// `(operator, operand)` pairs that follow.
///
/// Runs of the right-associative `..` or `^` nest on the right. Left nested,
/// each level would be its own group and re-run `fits` over its remainder,
/// which is quadratic on long concat chains. Splicing them in keeps the
/// emitted text the same and lays the run out like a left-associative chain.
fn collect_binary_chain(binary_op: &BinaryOp) -> (&Expression, Vec<(BinOp, &Expression)>) {
    let mut parts = Vec::new();
    let mut current = binary_op;
    loop {
        parts.push((current.op, &current.right));
        let Expression::BinaryOp(left_binary_op) = &current.left else {
            break;
        };
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
            flattened.push((op, &inner.left));
            op = inner.op;
            right = &inner.right;
        }
        flattened.push((op, right));
    }

    (&current.left, flattened)
}

impl Format for BinaryOp {
    fn fmt(&self, formatter: &mut Formatter) {
        let (first, chain) = collect_binary_chain(self);
        // One group over the whole chain, so it's either flat or breaks before
        // every operator.
        group((
            first,
            indent(format_with(|indented| {
                for (op, right_expr) in &chain {
                    (soft_line_or_space(), token(op.static_text()), space(), *right_expr)
                        .fmt(indented);
                }
            })),
        ))
        .fmt(formatter);
    }
}

/// `-` right before a nested unary `-` would form `--` and start a comment, so
/// the pair needs a space (`- -x`).
fn is_double_minus_hazard(op: UnOp, operand: &Expression) -> bool {
    op == UnOp::Neg && matches!(operand, Expression::UnaryOp(inner) if inner.op == UnOp::Neg)
}

impl Format for UnaryOp {
    fn fmt(&self, formatter: &mut Formatter) {
        token(self.op.static_text()).fmt(formatter);
        // `not` is a keyword operator: always separated from its operand.
        if self.op == UnOp::Not || is_double_minus_hazard(self.op, &self.operand) {
            space().fmt(formatter);
        }
        self.operand.fmt(formatter);
    }
}

impl Format for ParenthesizedExpression {
    fn fmt(&self, formatter: &mut Formatter) {
        (token("("), &self.expr, token(")")).fmt(formatter);
    }
}

impl Format for IfExpression {
    fn fmt(&self, formatter: &mut Formatter) {
        // Luau: one group so a long if-expression breaks before each branch.
        group(format_with(|grouped| {
            (
                token("if"),
                space(),
                &self.condition,
                space(),
                token("then"),
                indent((soft_line_or_space(), &self.then_expr)),
            )
                .fmt(grouped);

            for clause in &self.elseif_clauses {
                (
                    soft_line_or_space(),
                    token("elseif"),
                    space(),
                    &clause.condition,
                    space(),
                    token("then"),
                    indent((soft_line_or_space(), &clause.expr)),
                )
                    .fmt(grouped);
            }

            (soft_line_or_space(), token("else"), indent((soft_line_or_space(), &self.else_expr)))
                .fmt(grouped);
        }))
        .fmt(formatter);
    }
}

impl Format for InterpolatedString {
    fn fmt(&self, formatter: &mut Formatter) {
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
            formatter.write_text(piece);
            if let Some(expr) = &segment.expr {
                // Luau rejects `{{`, so an expression starting with `{`
                // gets a space after the opening brace, and one before the
                // closing brace to match.
                if expr_starts_with_brace(expr) {
                    (space(), expr, space()).fmt(formatter);
                } else {
                    expr.fmt(formatter);
                }
            }
            opener = segment.expr.is_some().then_some('}');
        }
        formatter.write_text("`");
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
    fn if_expression_flat() {
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
