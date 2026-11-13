//! Leaf text comes from token and literal values, never from source, so
//! synthetic ASTs stay printable.

use luck_ast::{
    BinaryOp, CallSuffix, Expression, IfExpression, InterpolatedString, ParenthesizedExpression,
    Suffix, SuffixedExpression, UnaryOp, Var, expr_starts_with_brace,
};
use luck_token::{CompactString, TokenKind, UnOp};

use crate::format_type::ExplicitTypeArgs;
use crate::ir::{
    Format, Formatter, format_with, group, indent, nested_groups, soft_line, soft_line_or_space,
    space, token,
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

            Self::Name(name) => name.fmt(f),
            Self::Suffixed(chain) => chain.fmt(f),
            Self::BinaryOp(binary_op) => binary_op.fmt(f),
            Self::UnaryOp(unary_op) => unary_op.fmt(f),
            Self::Parenthesized(parenthesized) => parenthesized.fmt(f),
            Self::TableConstructor(table) => table.fmt(f),

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
            Self::TypeCast(cast) => {
                (&cast.expr, space(), token("::"), space(), &cast.type_annotation).fmt(f);
            }

            // A parse-recovery placeholder has no text to print.
            Self::Error(_) => {}
        }
    }
}

impl Format for Var {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Name(name) => name.fmt(f),
            Self::Suffixed(chain) => chain.fmt(f),
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
            Expression::BinaryOp(binary_op) => &binary_op.first,
            // Luau
            Expression::TypeCast(cast) => &cast.expr,
            Expression::Suffixed(chain) => &chain.prefix,
            Expression::Name(_)
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

/// A chain lays out run by run, where a run is calls in a row, fields in a
/// row, or a lone index or instantiation.
///
/// Calls with two or more `:` methods among them break before each method
/// when the chain doesn't fit, in a group that starts at the chain's prefix.
/// Three or more fields break before each `.`, in a group of their own, so a
/// break before them, as in a callback passed earlier, doesn't break them.
impl Format for SuffixedExpression {
    fn fmt(&self, f: &mut Formatter) {
        let method_chains = runs(&self.suffixes).filter(|run| is_method_chain(run)).count();
        nested_groups(method_chains, |chained, groups| {
            self.prefix.fmt(chained);
            for run in runs(&self.suffixes) {
                match run {
                    [Suffix::Call(_), ..] if is_method_chain(run) => {
                        indent(format_with(|indented| {
                            for call in calls(run) {
                                if call.method.is_some() {
                                    soft_line().fmt(indented);
                                }
                                call.fmt(indented);
                            }
                        }))
                        .fmt(chained);
                        groups.end_innermost(chained);
                    }
                    [Suffix::Field(_), _, _, ..] => group(indent(format_with(|indented| {
                        for suffix in run {
                            if let Suffix::Field(name) = suffix {
                                (soft_line(), token("."), name).fmt(indented);
                            }
                        }
                    })))
                    .fmt(chained),
                    _ => {
                        for suffix in run {
                            write_suffix(chained, suffix);
                        }
                    }
                }
            }
        })
        .fmt(f);
    }
}

fn write_suffix(f: &mut Formatter, suffix: &Suffix) {
    match suffix {
        Suffix::Field(name) => (token("."), name).fmt(f),
        Suffix::Index { index, .. } => BracketedKey(index).fmt(f),
        Suffix::Call(call) => call.fmt(f),
        // Luau
        Suffix::TypeInstantiation { type_args, .. } => ExplicitTypeArgs(type_args).fmt(f),
    }
}

/// Calls in a row and fields in a row each make one run, and every other
/// suffix a run of its own.
fn runs(suffixes: &[Suffix]) -> impl Iterator<Item = &[Suffix]> {
    suffixes.chunk_by(|left, right| {
        matches!(
            (left, right),
            (Suffix::Call(_), Suffix::Call(_)) | (Suffix::Field(_), Suffix::Field(_))
        )
    })
}

fn calls(run: &[Suffix]) -> impl Iterator<Item = &CallSuffix> {
    run.iter().filter_map(|suffix| if let Suffix::Call(call) = suffix { Some(call) } else { None })
}

fn is_method_chain(run: &[Suffix]) -> bool {
    calls(run).filter(|call| call.method.is_some()).count() >= 2
}

/// An operand that binds tighter keeps a group of its own, so
/// `a == b or c == d` breaks at `or` alone.
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
    let operators = format_with(|lines| {
        for link in &binary_op.links {
            (soft_line_or_space(), token(link.op.static_text()), space(), &link.operand).fmt(lines);
        }
    });
    // One group over the whole run, so it's either flat or breaks before
    // every operator.
    if is_indented {
        group((&binary_op.first, indent(&operators))).fmt(f);
    } else {
        group((&binary_op.first, &operators)).fmt(f);
    }
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
        self.operand.fmt(f);
    }
}

impl Format for ParenthesizedExpression {
    fn fmt(&self, f: &mut Formatter) {
        (token("("), &self.expr, token(")")).fmt(f);
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
