//! Expression layout: `impl Format` over `luck_ast` expression nodes.
//!
//! Leaf text never slices source - identifiers/numbers/strings come from the
//! token-carried values via `tokens::write_token`, so synthetic ASTs format.

use luck_ast::expr::{
    BinaryOp, Expression, FieldAccess, IfExpression, InterpolatedString, ParenExpression, UnaryOp,
    Var,
};
use luck_token::{BinOp, Token, TokenKind, UnOp};

use crate::ir::*;
use crate::quotes::normalize_quote;
use crate::tokens::write_token;

impl Format for Expression {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            // Fixed-spelling leaves.
            Expression::Nil(_) => token("nil").fmt(f),
            Expression::False(_) => token("false").fmt(f),
            Expression::True(_) => token("true").fmt(f),
            Expression::VarArg(_) => token("...").fmt(f),

            Expression::Number(token) => format_number(f, token),
            Expression::StringLiteral(token) => format_string_literal(f, token),

            Expression::Var(var) => var.fmt(f),
            Expression::BinaryOp(binop) => binop.fmt(f),
            Expression::UnaryOp(unop) => unop.fmt(f),
            Expression::Parenthesized(paren) => paren.fmt(f),
            Expression::TableConstructor(table) => table.fmt(f),

            // Call/def bodies are owned by format_function.rs (another module);
            // an anonymous def is the `function` keyword plus its body.
            Expression::FunctionCall(call) => call.fmt(f),
            Expression::FunctionDef(def) => {
                token("function").fmt(f);
                crate::format_function::FormatFunctionBody { body: &def.body }.fmt(f);
            }

            Expression::IfExpression(if_expr) => if_expr.fmt(f), // Luau
            Expression::InterpolatedString(interp) => interp.fmt(f), // Luau
            Expression::TypeCast(cast) => {
                // Luau
                cast.expr.fmt(f);
                crate::write!(f, [space(), token("::"), space()]);
                cast.type_annotation.fmt(f);
            }

            // Parse-recovery placeholder: source is unslicable here, so there
            // is nothing faithful to emit.
            Expression::Error(_) => {}
        }
    }
}

/// Numeric literals: the base prefix and exponent markers are canonicalized
/// to lowercase, hex digit case follows the configured `hexadecimal_case`.
fn format_number(f: &mut Formatter, token: &Token) {
    if let TokenKind::Number(literal) = &token.kind {
        text(crate::numbers::normalize_number(
            literal,
            f.options.hexadecimal_case,
        ))
        .fmt(f);
    }
}

/// String literals: long-bracket forms pass through untouched (escape-bearing
/// raw length is unreliable); short forms are re-quoted to the configured style.
fn format_string_literal(f: &mut Formatter, token: &Token) {
    // The variant guarantees a `StringLiteral` kind; a malformed token carries
    // no faithful text to emit.
    if let TokenKind::StringLiteral(literal) = &token.kind {
        text(normalize_quote(literal, f.options.quote_style)).fmt(f);
    }
}

impl Format for Var {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Var::Name(token) => write_token(f, token),
            Var::FieldAccess(access) => format_field_access(f, access),
            Var::Index(index) => {
                index.prefix.fmt(f);
                token("[").fmt(f);
                if starts_with_bracket(&index.index) {
                    space().fmt(f);
                    index.index.fmt(f);
                    space().fmt(f);
                } else {
                    index.index.fmt(f);
                }
                token("]").fmt(f);
            }
        }
    }
}

/// Whether the expression's leftmost rendered token begins with `[`: a
/// long-bracket string on the left spine. Emitting it directly after an
/// opening `[` would lex as a long-string opener (`[[`), so index and
/// bracket-field sites pad with spaces when this returns true.
pub(crate) fn starts_with_bracket(expr: &Expression) -> bool {
    match expr {
        Expression::StringLiteral(token) => match &token.kind {
            TokenKind::StringLiteral(literal) => literal.starts_with('['),
            _ => false,
        },
        Expression::BinaryOp(binop) => starts_with_bracket(&binop.left),
        Expression::TypeCast(cast) => starts_with_bracket(&cast.expr),
        Expression::FunctionCall(call) => starts_with_bracket(&call.callee),
        Expression::Var(var) => match var.as_ref() {
            Var::Name(_) => false,
            Var::Index(index) => starts_with_bracket(&index.prefix),
            Var::FieldAccess(access) => starts_with_bracket(&access.prefix),
        },
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
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
        match var.as_ref() {
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
fn format_field_access(f: &mut Formatter, access: &FieldAccess) {
    let (root, names) = collect_access_chain(access);
    if names.len() >= 3 {
        group(format_with(|f| {
            root.fmt(f);
            indent(format_with(|f| {
                for name in &names {
                    crate::write!(f, [soft_line(), token(".")]);
                    write_token(f, name);
                }
            }))
            .fmt(f);
        }))
        .fmt(f);
    } else {
        access.prefix.fmt(f);
        token(".").fmt(f);
        write_token(f, &access.name);
    }
}

/// Flatten a left-recursive binary chain into its leftmost operand and the
/// following `(operator, operand)` pairs.
///
/// Right-associative same-operator runs (`..`, `^`) nest on the right; each
/// nested level would otherwise become its own group, and every group re-runs
/// `fits` over its remainder - quadratic on long concat chains. Splicing them
/// into the flat chain keeps operand/operator order (and thus the emitted
/// text) identical while laying the chain out like a left-associative one.
fn collect_binary_chain(binop: &BinaryOp) -> (&Expression, Vec<(BinOp, &Expression)>) {
    let mut parts = Vec::new();
    let mut current = binop;
    loop {
        parts.push((current.op, &current.right));
        match &current.left {
            Expression::BinaryOp(left_binop) => current = left_binop,
            _ => break,
        }
    }
    parts.reverse();

    let mut flattened: Vec<(BinOp, &Expression)> = Vec::with_capacity(parts.len());
    for (mut op, mut right) in parts {
        while let Expression::BinaryOp(inner) = right {
            let same_right_assoc_op = matches!(
                (op, inner.op),
                (BinOp::Concat, BinOp::Concat) | (BinOp::Pow, BinOp::Pow)
            );
            if !same_right_assoc_op {
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
    fn fmt(&self, f: &mut Formatter) {
        let (first, chain) = collect_binary_chain(self);
        // One group over the whole chain: it prints flat if it fits, otherwise
        // every operator moves to its own indented line (break before the op).
        group(format_with(|f| {
            first.fmt(f);
            indent(format_with(|f| {
                for (op, right_expr) in &chain {
                    soft_line_or_space().fmt(f);
                    token(op.static_text()).fmt(f);
                    space().fmt(f);
                    right_expr.fmt(f);
                }
            }))
            .fmt(f);
        }))
        .fmt(f);
    }
}

/// `-` immediately before a nested unary `-` would form `--`, starting a
/// comment; a space keeps the output re-parseable (`- -x`).
fn is_double_minus_hazard(op: UnOp, operand: &Expression) -> bool {
    op == UnOp::Neg && matches!(operand, Expression::UnaryOp(inner) if inner.op == UnOp::Neg)
}

impl Format for UnaryOp {
    fn fmt(&self, f: &mut Formatter) {
        token(self.op.static_text()).fmt(f);
        if self.op == UnOp::Not {
            // Keyword operator: always separated from its operand.
            space().fmt(f);
        } else if is_double_minus_hazard(self.op, &self.operand) {
            space().fmt(f);
        }
        self.operand.fmt(f);
    }
}

impl Format for ParenExpression {
    fn fmt(&self, f: &mut Formatter) {
        token("(").fmt(f);
        self.expr.fmt(f);
        token(")").fmt(f);
    }
}

impl Format for IfExpression {
    fn fmt(&self, f: &mut Formatter) {
        // Luau: one group so a long if-expression breaks before each branch.
        group(format_with(|f| {
            crate::write!(f, [token("if"), space()]);
            self.condition.fmt(f);
            crate::write!(f, [space(), token("then")]);
            indent(format_with(|f| {
                soft_line_or_space().fmt(f);
                self.then_expr.fmt(f);
            }))
            .fmt(f);

            for clause in &self.elseif_clauses {
                crate::write!(f, [soft_line_or_space(), token("elseif"), space()]);
                clause.condition.fmt(f);
                crate::write!(f, [space(), token("then")]);
                indent(format_with(|f| {
                    soft_line_or_space().fmt(f);
                    clause.expr.fmt(f);
                }))
                .fmt(f);
            }

            crate::write!(f, [soft_line_or_space(), token("else")]);
            indent(format_with(|f| {
                soft_line_or_space().fmt(f);
                self.else_expr.fmt(f);
            }))
            .fmt(f);
        }))
        .fmt(f);
    }
}

impl Format for InterpolatedString {
    fn fmt(&self, f: &mut Formatter) {
        // Luau: each segment is a literal part (its backtick/brace punctuation
        // is added by write_token) optionally followed by an embedded expr.
        for segment in &self.segments {
            write_token(f, &segment.literal);
            if let Some(expr) = &segment.expr {
                expr.fmt(f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::Comments;
    use crate::printer::{PrinterOptions, print};
    use luck_ast::stmt::LastStatement;
    use luck_token::LuaVersion;

    fn print_expr(source: &str, quote_style: crate::QuoteStyle, line_width: u16) -> String {
        let wrapped = format!("return {source}\n");
        let parsed = luck_parser::parse(&wrapped, LuaVersion::Luau);
        let last = parsed.block.last_stmt.expect("wrapped return statement");
        let LastStatement::Return(ret) = last.as_ref() else {
            panic!("expected a return statement");
        };
        let expr = &ret.exprs.items[0].0;

        let options = crate::FormatOptions {
            quote_style,
            line_width,
            ..crate::FormatOptions::default()
        };
        let mut formatter = Formatter::with_context(options, Comments::none());
        expr.fmt(&mut formatter);
        let group_count = formatter.group_count();
        let elements = formatter.into_elements();
        print(
            &elements,
            group_count,
            &PrinterOptions {
                line_width,
                use_tabs: false,
                indent_width: 4,
            },
        )
    }

    #[test]
    fn string_requoted_to_double() {
        assert_eq!(print_expr("'hi'", crate::QuoteStyle::Double, 80), "\"hi\"");
    }

    #[test]
    fn long_bracket_string_untouched() {
        assert_eq!(
            print_expr("[[raw]]", crate::QuoteStyle::Double, 80),
            "[[raw]]"
        );
    }

    #[test]
    fn binary_chain_flat_fits() {
        assert_eq!(
            print_expr("1 + 2 + 3", crate::QuoteStyle::Double, 80),
            "1 + 2 + 3"
        );
    }

    #[test]
    fn binary_chain_breaks_before_operator() {
        let output = print_expr("aaaa + bbbb + cccc", crate::QuoteStyle::Double, 10);
        assert!(output.starts_with("aaaa"));
        assert!(output.contains('\n'));
        assert!(output.contains("+ bbbb"));
    }

    #[test]
    fn unary_not_keeps_keyword_space() {
        assert_eq!(print_expr("not x", crate::QuoteStyle::Double, 80), "not x");
    }

    #[test]
    fn unary_minus_no_space() {
        assert_eq!(print_expr("-x", crate::QuoteStyle::Double, 80), "-x");
    }

    #[test]
    fn nested_unary_minus_gets_space() {
        // Must not collapse to `--x` (a comment).
        assert_eq!(print_expr("- -x", crate::QuoteStyle::Double, 80), "- -x");
    }

    #[test]
    fn short_field_access_stays_glued() {
        assert_eq!(print_expr("a.b.c", crate::QuoteStyle::Double, 80), "a.b.c");
    }

    #[test]
    fn long_field_chain_breaks_per_segment() {
        let output = print_expr("aaaa.bbbb.cccc.dddd", crate::QuoteStyle::Double, 10);
        assert!(output.starts_with("aaaa"));
        assert!(output.contains(".bbbb"));
        assert!(output.contains('\n'));
    }

    #[test]
    fn parenthesized_preserved() {
        assert_eq!(
            print_expr("(1 + 2)", crate::QuoteStyle::Double, 80),
            "(1 + 2)"
        );
    }

    #[test]
    fn if_expression_flat() {
        assert_eq!(
            print_expr("if a then b else c", crate::QuoteStyle::Double, 80),
            "if a then b else c"
        );
    }

    #[test]
    fn interpolated_string_roundtrips() {
        assert_eq!(
            print_expr("`hi {x} there`", crate::QuoteStyle::Double, 80),
            "`hi {x} there`"
        );
    }
}
