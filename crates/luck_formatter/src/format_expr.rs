//! Leaf text comes from the tree's leaves, never from source, so synthetic
//! ASTs stay printable.

use core::iter;

use luck_ast::{
    BinaryOp, Expression, FunctionDef, GetSpan as _, IfExpression, InterpolatedString, Suffix,
    SuffixedExpression, UnaryOp, Var, expr_starts_with_brace,
};
use luck_token::UnOp;

use crate::ir::Formatter;
use crate::numbers::normalize_number;
use crate::quotes::requote;

fn starts_with_bracket(expr: &Expression<'_>) -> bool {
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

const fn is_call(suffix: &Suffix<'_>) -> bool {
    matches!(suffix, Suffix::Call(_) | Suffix::MethodCall(_))
}

/// Calls in a row and fields in a row each make one run, and every other
/// suffix a run of its own.
fn runs<'chain, 'ast>(
    suffixes: &'chain [Suffix<'ast>],
) -> impl Iterator<Item = &'chain [Suffix<'ast>]> {
    suffixes.chunk_by(|left, right| {
        (is_call(left) && is_call(right))
            || matches!((left, right), (Suffix::Field(_), Suffix::Field(_)))
    })
}

fn is_method_chain(run: &[Suffix<'_>]) -> bool {
    run.iter().filter(|suffix| matches!(suffix, Suffix::MethodCall(_))).count() >= 2
}

/// `-` right before a nested unary `-` would form `--` and start a comment, so
/// the pair needs a space (`- -x`).
fn is_double_minus_hazard(op: UnOp, operand: &Expression<'_>) -> bool {
    op == UnOp::Neg && matches!(operand, Expression::UnaryOp(inner) if inner.op == UnOp::Neg)
}

impl<'ast> Formatter<'ast> {
    pub(crate) fn write_expr(&mut self, expr: &Expression<'ast>) {
        self.node(
            || expr.span(),
            |f| match expr {
                Expression::Nil(_) => f.token("nil"),
                Expression::False(_) => f.token("false"),
                Expression::True(_) => f.token("true"),
                Expression::VarArg(_) => f.token("..."),
                Expression::Number(literal) | Expression::Integer(literal) => {
                    f.write_number(literal.text);
                }
                Expression::StringLiteral(literal) => f.write_string(literal.text),
                Expression::Name(name) => f.ascii(name.text.as_str()),
                Expression::Suffixed(chain) => f.write_suffixed(chain),
                Expression::BinaryOp(binary_op) => f.write_binary_chain(binary_op, true),
                Expression::UnaryOp(unary_op) => f.write_unary(unary_op),
                Expression::Parenthesized(parenthesized) => {
                    f.token("(");
                    f.write_expr(&parenthesized.expr);
                    f.write_closing_comments(parenthesized.span.end, false);
                    f.token(")");
                }
                Expression::TableConstructor(table) => f.write_table(table, false),
                Expression::FunctionDef(function_def) => {
                    f.without_sorting_requires(|unsorted| {
                        unsorted.write_function_definition(function_def);
                    });
                }
                // Luau
                Expression::IfExpression(if_expr) => f.write_if_expr(if_expr),
                // Luau
                Expression::InterpolatedString(interpolated) => f.write_interpolated(interpolated),
                // Luau
                Expression::TypeCast(cast) => {
                    f.write_expr(&cast.expr);
                    f.space();
                    f.token("::");
                    f.space();
                    f.write_type(&cast.ty);
                }
                // A parse-recovery placeholder has no text to print.
                Expression::Error(_) => {}
            },
        );
    }

    pub(crate) fn write_number(&mut self, text: &'ast str) {
        let hex_case = self.options().hexadecimal_case;
        let (buffer, allocator) = self.scratch();
        let normalized = if normalize_number(text, hex_case, buffer) {
            allocator.alloc_str(buffer)
        } else {
            text
        };
        self.ascii(normalized);
    }

    pub(crate) fn write_string(&mut self, text: &'ast str) {
        let quote_style = self.options().quote_style;
        let (buffer, allocator) = self.scratch();
        let requoted =
            if requote(text, quote_style, buffer) { allocator.alloc_str(buffer) } else { text };
        self.text(requoted);
    }

    /// A function expression. Only one a statement assigns, which prints
    /// this directly, keeps the require sorting of the block around it.
    pub(crate) fn write_function_definition(&mut self, function_def: &FunctionDef<'ast>) {
        // Luau attributes stay on the same line as `function` here.
        for attribute in &function_def.attributes {
            self.write_function_attribute(attribute);
            self.space();
        }
        self.token("function");
        self.write_function_body(&function_def.body);
    }

    pub(crate) fn write_var(&mut self, var: &Var<'ast>) {
        match var {
            Var::Name(name) => self.name(**name),
            Var::Suffixed(chain) => self.node(|| chain.span, |f| f.write_suffixed(chain)),
        }
    }

    /// Right after the opening `[`, a long-bracket string would lex as `[[`,
    /// a long-string opener, so it's padded with spaces. `closer` is where
    /// the `]` is.
    pub(crate) fn write_bracketed_key(&mut self, key: &Expression<'ast>, closer: u32) {
        let is_padded = starts_with_bracket(key);
        self.token("[");
        if is_padded {
            self.space();
        }
        self.write_expr(key);
        self.write_closing_comments(closer, false);
        if is_padded {
            self.space();
        }
        self.token("]");
    }

    /// A chain lays out run by run, where a run is calls in a row, fields in
    /// a row, or a lone index or instantiation.
    ///
    /// Calls with two or more `:` methods among them break before each
    /// method, and three or more fields before each `.`, when the run doesn't
    /// fit. Each run breaks in a group of its own, so a break before it, as in
    /// a callback passed earlier, doesn't break it.
    ///
    /// A comment on a line of its own before a suffix goes above it, where
    /// the chain breaks. One before call arguments goes inside them.
    pub(crate) fn write_suffixed(&mut self, chain: &SuffixedExpression<'ast>) {
        self.write_expr(&chain.prefix);
        for run in runs(&chain.suffixes) {
            match run {
                [first, ..] if is_call(first) && is_method_chain(run) => {
                    self.group(|grouped| {
                        grouped.indent(|f| {
                            for suffix in run {
                                if matches!(suffix, Suffix::MethodCall(_)) {
                                    f.soft_line();
                                    f.write_hoisted_comments(suffix.span().start);
                                }
                                f.write_suffix(suffix);
                            }
                        });
                    });
                }
                [Suffix::Field(_), _, _, ..] => self.group(|grouped| {
                    grouped.indent(|f| {
                        for suffix in run {
                            if let Suffix::Field(name) = suffix {
                                f.soft_line();
                                f.write_hoisted_comments(name.span.start);
                                f.token(".");
                                f.name(*name);
                            }
                        }
                    });
                }),
                _ => {
                    for suffix in run {
                        if !matches!(suffix, Suffix::Call(_))
                            && self.comments().has_line_ending_comment_before(suffix.span().start)
                        {
                            self.space();
                            self.write_leading_comments(suffix.span().start);
                        }
                        self.write_suffix(suffix);
                    }
                }
            }
        }
    }

    fn write_suffix(&mut self, suffix: &Suffix<'ast>) {
        match suffix {
            Suffix::Field(name) => {
                self.token(".");
                self.name(*name);
            }
            Suffix::Index { span, index } => self.write_bracketed_key(index, span.end),
            Suffix::Call(args) => self.write_function_args(args),
            Suffix::MethodCall(call) => {
                self.token(":");
                self.name(call.name);
                // Luau rejects a method call whose `(` is on a later line
                // than its name. The tree keeps no span for the outer `>`.
                if let Some(type_args) = &call.type_args {
                    let closer = self.comments().closer_offset(type_args.span.end, b'>');
                    self.flat(|f| f.write_type_args(("<<", ">>"), type_args, closer));
                }
                self.write_function_args(&call.args);
            }
            // Luau
            Suffix::TypeInstantiation { span, type_args } => {
                self.write_type_args(("<<", ">>"), type_args, span.end);
            }
        }
    }

    /// One group over the whole run, so it's either flat or breaks before
    /// every operator. An operand that binds tighter keeps a group of its
    /// own, so `a == b or c == d` breaks at `or` alone.
    ///
    /// Unless `is_indented`, a broken chain keeps its operators level with
    /// its first operand, for a condition that already sits on an indented
    /// line of its own.
    ///
    /// A comment that ends its line before an operand goes above the operator,
    /// which starts the operand's line when the chain breaks.
    pub(crate) fn write_binary_chain(&mut self, binary_op: &BinaryOp<'ast>, is_indented: bool) {
        let operators = |f: &mut Self| {
            for link in &binary_op.links {
                f.soft_line_or_space();
                f.write_hoisted_comments(link.operand.span().start);
                f.token(link.op.static_text());
                f.space();
                f.write_expr(&link.operand);
            }
        };
        self.group(|f| {
            f.write_expr(&binary_op.first);
            if is_indented {
                f.indent(operators);
            } else {
                operators(f);
            }
        });
    }

    /// `not` is a keyword operator, always separated from its operand, and a
    /// comment right after `-` would start with `---`.
    fn write_unary(&mut self, unary_op: &UnaryOp<'ast>) {
        self.token(unary_op.op.static_text());
        let operand = &unary_op.operand;
        if unary_op.op == UnOp::Not
            || is_double_minus_hazard(unary_op.op, operand)
            || (unary_op.op == UnOp::Neg
                && self.comments().has_comment_before(operand.span().start))
        {
            self.space();
        }
        self.write_expr(operand);
    }

    /// Luau: one group, so a long if-expression starts an indented line at
    /// each `then`, `elseif`, and `else`, above which goes a comment ending
    /// its line before the expression after it.
    fn write_if_expr(&mut self, if_expr: &IfExpression<'ast>) {
        self.group(|grouped| {
            grouped.token("if");
            grouped.space();
            grouped.write_expr(&if_expr.condition);
            grouped.indent(|f| {
                f.soft_line_or_space();
                f.write_hoisted_comments(if_expr.then_expr.span().start);
                f.token("then");
                f.space();
                f.write_expr(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    f.soft_line_or_space();
                    f.write_hoisted_comments(clause.condition.span().start);
                    f.token("elseif");
                    f.space();
                    f.write_expr(&clause.condition);
                    f.space();
                    f.token("then");
                    f.space();
                    f.write_expr(&clause.expr);
                }
                f.soft_line_or_space();
                f.write_hoisted_comments(if_expr.else_expr.span().start);
                f.token("else");
                f.space();
                f.write_expr(&if_expr.else_expr);
            });
        });
    }

    /// Luau. Luau rejects `{{`, so an expression starting with `{` gets a
    /// space after the opening brace, and one before the closing brace to
    /// match. A hole's `}` comes right before the piece after it.
    fn write_interpolated(&mut self, interpolated: &InterpolatedString<'ast>) {
        self.token("`");
        let closers = interpolated
            .segments
            .iter()
            .skip(1)
            .map(|segment| segment.text.span.start)
            .chain(iter::once(interpolated.tail.span.start));
        for (segment, closer) in interpolated.segments.iter().zip(closers) {
            if !segment.text.text.is_empty() {
                self.text(segment.text.text);
            }
            self.token("{");
            let is_padded = expr_starts_with_brace(&segment.expr);
            if is_padded {
                self.space();
            }
            self.write_expr(&segment.expr);
            self.write_closing_comments(closer, false);
            if is_padded {
                self.space();
            }
            self.token("}");
        }
        if !interpolated.tail.text.is_empty() {
            self.text(interpolated.tail.text);
        }
        self.token("`");
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::render_expr;
    use crate::{FormatOptions, IndentStyle, QuoteStyle};

    fn print_expr(source: &str, quote_style: QuoteStyle, line_width: u16) -> String {
        let options = FormatOptions {
            line_width,
            indent_style: IndentStyle::Spaces,
            quote_style,
            ..FormatOptions::default()
        };
        render_expr(source, options)
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
        assert_eq!(
            print_expr("aaaa + bbbb + cccc", QuoteStyle::Double, 10),
            "\
aaaa
    + bbbb
    + cccc"
        );
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
        assert_eq!(
            print_expr("aaaa.bbbb.cccc.dddd", QuoteStyle::Double, 10),
            "\
aaaa
    .bbbb
    .cccc
    .dddd"
        );
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
        // A plain backtick string has no expression, and must not gain a
        // spurious `{}` hole.
        assert_eq!(print_expr("`plain text`", QuoteStyle::Double, 80), "`plain text`");
    }
}
