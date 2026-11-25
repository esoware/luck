//! Leaf text comes from the tree's leaves, never from source, so synthetic
//! ASTs stay printable.
//!
//! An expression takes the parentheses it needs where it goes, and keeps the
//! ones the source grouped it in where they show how the user groups an
//! operator expression, or where a comment sits in them. Others come off,
//! as `StyLua`'s do.

use std::iter;

use luck_ast::{
    BinaryOp, Expression, FunctionDef, GetSpan as _, IfExpression, InterpolatedString,
    NumberLiteral, Slot, StringLiteral, Suffix, SuffixedExpression, UnaryOp, Var,
};
use luck_token::{Span, TokenKind, UnOp, spell_number, spell_string, write_interpolated_text};

use crate::format_table::TablePlacement;
use crate::ir::Formatter;
use crate::numbers::normalize_number;
use crate::quotes::requote;

/// Where a broken operator chain puts the lines after its first.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ChainIndent {
    /// Level with its first operand, for a chain that already sits on an
    /// indented line of its own.
    Level,
    Indented,
}

/// A unary, binary, if, or cast expression, whose own parentheses show how
/// a user groups it.
const fn is_operator_expr(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::TypeCast(_) => true,
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => false,
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

impl<'ast> Formatter<'ast> {
    /// An expression where nothing groups with it.
    pub(crate) fn write_expr(&mut self, expr: &Expression<'ast>) {
        self.write_expr_in(expr, Slot::FREE);
    }

    /// `expr` in `slot`, in parentheses where it needs them there or keeps
    /// the source's. Either way the parentheses the source wrote bound its
    /// comments, so those before them lead it and those after them trail it.
    pub(crate) fn write_expr_in(&mut self, expr: &Expression<'ast>, slot: Slot) {
        let grouping = self.grouping(expr);
        let is_kept = grouping.is_some_and(|parens| self.keeps_grouping(expr, parens));
        let span = grouping.unwrap_or_else(|| expr.span());
        if is_kept {
            self.node(
                || span,
                |f| {
                    f.token("(");
                    f.node(|| expr.span(), |inner| inner.write_bare_expr(expr, Slot::FREE));
                    f.write_closing_comments(span.end);
                    f.token(")");
                },
            );
        } else if expr.needs_parens(slot) {
            self.node(
                || span,
                |f| {
                    f.token("(");
                    f.write_bare_expr(expr, Slot::FREE);
                    f.token(")");
                },
            );
        } else {
            self.node(|| span, |f| f.write_bare_expr(expr, slot));
        }
    }

    /// Whether `expr` in `slot` prints in parentheses.
    pub(crate) fn is_wrapped(&self, expr: &Expression<'ast>, slot: Slot) -> bool {
        expr.needs_parens(slot)
            || self.grouping(expr).is_some_and(|parens| self.keeps_grouping(expr, parens))
    }

    /// The span of the parentheses the source grouped `expr` in, where they
    /// can matter: kept around an operator expression, or bounding a comment.
    fn grouping(&self, expr: &Expression<'ast>) -> Option<Span> {
        if !is_operator_expr(expr) && !self.comments().has_pending_before(self.enclosing_end()) {
            return None;
        }
        self.grouping_around(expr.span())
    }

    /// Where `expr` starts and ends, the parentheses the source grouped it in
    /// included where they can matter.
    pub(crate) fn extent(&self, expr: &Expression<'ast>) -> Span {
        self.grouping(expr).unwrap_or_else(|| expr.span())
    }

    /// Whether the `parens` the source grouped `expr` in stay: around an
    /// operator expression, or around a comment outside it, which needs the
    /// line breaks they hold.
    fn keeps_grouping(&self, expr: &Expression<'ast>, parens: Span) -> bool {
        is_operator_expr(expr)
            || self.comments().has_comment_outside(parens, iter::once(expr.span()))
    }

    /// What `expr` in `slot` prints first, or `None` when that's a
    /// parenthesis.
    fn first_printed<'tree>(
        &self,
        expr: &'tree Expression<'ast>,
        slot: Slot,
    ) -> Option<&'tree Expression<'ast>> {
        let (mut current, mut current_slot) = (expr, slot);
        loop {
            if matches!(current, Expression::Parenthesized(_))
                || self.is_wrapped(current, current_slot)
            {
                return None;
            }
            (current, current_slot) = match current {
                Expression::Suffixed(chain) => (&chain.prefix, Slot::PREFIX),
                Expression::BinaryOp(binary_op) => {
                    (&binary_op.first, current_slot.operand(binary_op, 0))
                }
                Expression::TypeCast(cast) => (&cast.expr, Slot::CAST_OPERAND),
                Expression::Nil(_)
                | Expression::False(_)
                | Expression::True(_)
                | Expression::Number(_)
                | Expression::StringLiteral(_)
                | Expression::VarArg(_)
                | Expression::FunctionDef(_)
                | Expression::Name(_)
                | Expression::Parenthesized(_)
                | Expression::TableConstructor(_)
                | Expression::UnaryOp(_)
                | Expression::IfExpression(_)
                | Expression::InterpolatedString(_)
                | Expression::Error(_) => return Some(current),
            };
        }
    }

    /// Whether the statement-starting `prefix` prints in parentheses, which
    /// a `;` keeps from calling the statement before.
    pub(crate) fn starts_with_paren(&self, prefix: &Expression<'ast>) -> bool {
        matches!(prefix, Expression::Parenthesized(_)) || self.is_wrapped(prefix, Slot::PREFIX)
    }

    /// `expr` without parentheses of its own, its operands in the slots its
    /// own `slot` gives them.
    pub(crate) fn write_bare_expr(&mut self, expr: &Expression<'ast>, slot: Slot) {
        match expr {
            Expression::Nil(_) => self.token("nil"),
            Expression::False(_) => self.token("false"),
            Expression::True(_) => self.token("true"),
            Expression::VarArg(_) => self.token("..."),
            Expression::Number(literal) => self.write_number(literal),
            Expression::StringLiteral(literal) => self.write_string(literal),
            Expression::Name(name) => self.ascii(name.text.as_bytes()),
            Expression::Suffixed(chain) => self.write_suffixed(chain),
            Expression::BinaryOp(binary_op) => {
                self.write_binary_chain(binary_op, ChainIndent::Indented, slot);
            }
            Expression::UnaryOp(unary_op) => self.write_unary(unary_op, slot),
            Expression::Parenthesized(parenthesized) => {
                self.token("(");
                self.write_expr(&parenthesized.expr);
                self.write_closing_comments(parenthesized.span.end);
                self.token(")");
            }
            Expression::TableConstructor(table) => {
                self.write_table(table, TablePlacement::Standalone);
            }
            Expression::FunctionDef(function_def) => {
                self.without_sorting_requires(|unsorted| {
                    unsorted.write_function_definition(function_def);
                });
            }
            Expression::IfExpression(if_expr) => self.write_if_expr(if_expr),
            Expression::InterpolatedString(interpolated) => self.write_interpolated(interpolated),
            Expression::TypeCast(cast) => {
                self.write_expr_in(&cast.expr, Slot::CAST_OPERAND);
                self.space();
                self.token("::");
                self.space();
                self.write_type(&cast.ty);
            }
            // A parse-recovery placeholder has no text to print.
            Expression::Error(_) => {}
        }
    }

    /// The numeral as written, or for one built in code, the shortest that
    /// spells its value, normalized either way.
    fn write_number(&mut self, literal: &NumberLiteral<'ast>) {
        let text = if let Some(raw) = literal.raw {
            raw.as_bytes()
        } else {
            &*self.allocator().alloc_slice_copy(spell_number(literal.value).as_str().as_bytes())
        };
        self.write_numeral(text);
    }

    /// `text` is a numeral, which is ASCII.
    fn write_numeral(&mut self, text: &'ast [u8]) {
        let hex_case = self.options().hexadecimal_case;
        let (buffer, allocator) = self.scratch();
        let normalized = if normalize_number(text, hex_case, buffer) {
            allocator.alloc_slice_copy(buffer)
        } else {
            text
        };
        self.ascii(normalized);
    }

    /// The literal as written, or for one built in code, the shortest that
    /// spells its value, requoted either way.
    pub(crate) fn write_string(&mut self, literal: &StringLiteral<'ast>) {
        let text = self.string_spelling(literal);
        let quote_style = self.options().quote_style;
        let (buffer, allocator) = self.scratch();
        let requoted = if requote(text, quote_style, buffer) {
            allocator.alloc_slice_copy(buffer)
        } else {
            text
        };
        self.text(requoted);
    }

    fn string_spelling(&mut self, literal: &StringLiteral<'ast>) -> &'ast [u8] {
        if let Some(raw) = literal.raw {
            return raw.as_bytes();
        }
        let (buffer, allocator) = self.scratch();
        spell_string(&literal.value).write_to(buffer);
        allocator.alloc_slice_copy(buffer)
    }

    /// A piece of an interpolated string, as written or escaped from its
    /// value.
    fn write_interpolated_text(&mut self, text: &StringLiteral<'ast>) {
        let spelling = if let Some(raw) = text.raw {
            raw.as_bytes()
        } else {
            let (buffer, allocator) = self.scratch();
            write_interpolated_text(&text.value, buffer);
            &*allocator.alloc_slice_copy(buffer)
        };
        if !spelling.is_empty() {
            self.text(spelling);
        }
    }

    /// A function expression. Only one a statement assigns, which prints
    /// this directly, keeps the require sorting of the block around it.
    pub(crate) fn write_function_definition(&mut self, function_def: &FunctionDef<'ast>) {
        // Attributes stay on the same line as `function` here.
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
        let is_padded = matches!(
            self.first_printed(key, Slot::FREE),
            Some(Expression::StringLiteral(literal))
                if self.string_spelling(literal).first() == Some(&b'[')
        );
        self.token("[");
        if is_padded {
            self.space();
        }
        self.write_expr(key);
        self.write_closing_comments(closer);
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
        self.write_expr_in(&chain.prefix, Slot::PREFIX);
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
                // A method call whose `(` is on a later line than its name
                // doesn't parse. The tree keeps no span for the outer `>`.
                if let Some(type_args) = &call.type_args {
                    let closer =
                        self.comments().closer_offset(type_args.span.end, TokenKind::Greater);
                    self.flat(|f| f.write_type_args(("<<", ">>"), type_args, closer));
                }
                self.write_function_args(&call.args);
            }
            Suffix::TypeInstantiation { span, type_args } => {
                self.write_type_args(("<<", ">>"), type_args, span.end);
            }
        }
    }

    /// One group over the whole run, so it's either flat or breaks before
    /// every operator. An operand that binds tighter keeps a group of its
    /// own, so `a == b or c == d` breaks at `or` alone.
    ///
    /// A comment that ends its line before an operand goes above the operator,
    /// which starts the operand's line when the chain breaks.
    pub(crate) fn write_binary_chain(
        &mut self,
        binary_op: &BinaryOp<'ast>,
        indent: ChainIndent,
        slot: Slot,
    ) {
        let operators = |f: &mut Self| {
            for (index, link) in (1..).zip(&binary_op.links) {
                f.soft_line_or_space();
                f.write_hoisted_comments(f.extent(&link.operand).start);
                f.token(link.op.static_text());
                f.space();
                f.write_expr_in(&link.operand, slot.operand(binary_op, index));
            }
        };
        self.group(|f| {
            f.write_expr_in(&binary_op.first, slot.operand(binary_op, 0));
            match indent {
                ChainIndent::Level => operators(f),
                ChainIndent::Indented => f.indent(operators),
            }
        });
    }

    /// `not` is a keyword operator, always separated from its operand, `-`
    /// right before a bare `-` would form `--` and start a comment, and so
    /// would a comment right after `-` start with `---`.
    fn write_unary(&mut self, unary_op: &UnaryOp<'ast>, slot: Slot) {
        self.token(unary_op.op.static_text());
        let operand = &unary_op.operand;
        let operand_slot = slot.unary_operand();
        let is_spaced = match unary_op.op {
            UnOp::Not => true,
            UnOp::Neg => {
                (matches!(operand, Expression::UnaryOp(inner) if inner.op == UnOp::Neg)
                    && !self.is_wrapped(operand, operand_slot))
                    || self.comments().has_comment_before(self.extent(operand).start)
            }
            UnOp::Len => false,
        };
        if is_spaced {
            self.space();
        }
        self.write_expr_in(operand, operand_slot);
    }

    /// One group, so a long if-expression starts an indented line at
    /// each `then`, `elseif`, and `else`, above which goes a comment ending
    /// its line before the expression after it.
    fn write_if_expr(&mut self, if_expr: &IfExpression<'ast>) {
        self.group(|grouped| {
            grouped.token("if");
            grouped.space();
            if let Some(binding) = &if_expr.binding {
                grouped.write_conditional_binding(binding);
                grouped.space();
            }
            grouped.write_expr(&if_expr.condition);
            grouped.indent(|f| {
                f.soft_line_or_space();
                f.write_hoisted_comments(f.extent(&if_expr.then_expr).start);
                f.token("then");
                f.space();
                f.write_expr(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    f.soft_line_or_space();
                    let start = clause.binding.as_ref().map_or_else(
                        || f.extent(&clause.condition).start,
                        |binding| binding.span.start,
                    );
                    f.write_hoisted_comments(start);
                    f.token("elseif");
                    f.space();
                    if let Some(binding) = &clause.binding {
                        f.write_conditional_binding(binding);
                        f.space();
                    }
                    f.write_expr(&clause.condition);
                    f.space();
                    f.token("then");
                    f.space();
                    f.write_expr(&clause.expr);
                }
                f.soft_line_or_space();
                f.write_hoisted_comments(f.extent(&if_expr.else_expr).start);
                f.token("else");
                f.space();
                f.write_expr(&if_expr.else_expr);
            });
        });
    }

    /// `{{` is an error in an interpolated string, so an expression starting
    /// with `{` gets a space after the opening brace, and one before the
    /// closing brace to match. A hole's `}` comes right before the piece
    /// after it.
    fn write_interpolated(&mut self, interpolated: &InterpolatedString<'ast>) {
        self.token("`");
        let closers = interpolated
            .segments
            .iter()
            .skip(1)
            .map(|segment| segment.text.span.start)
            .chain(iter::once(interpolated.tail.span.start));
        for (segment, closer) in interpolated.segments.iter().zip(closers) {
            self.write_interpolated_text(&segment.text);
            self.token("{");
            let is_padded = matches!(
                self.first_printed(&segment.expr, Slot::FREE),
                Some(Expression::TableConstructor(_))
            );
            if is_padded {
                self.space();
            }
            self.write_expr(&segment.expr);
            self.write_closing_comments(closer);
            if is_padded {
                self.space();
            }
            self.token("}");
        }
        self.write_interpolated_text(&interpolated.tail);
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
