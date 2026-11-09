//! Function bodies, calls, and their argument and parameter lists.

use luck_ast::{
    Expression, FunctionArgs, FunctionAttribute, FunctionBody, FunctionCall, Literal, Parameter,
    Punctuated, TypeArgs, VarArgParam, ensure_sufficient_stack,
};
use luck_token::Token;

use crate::comments::{ListSeparator, write_commented_list, write_dangling_comments};
use crate::format_type::ExplicitTypeArgs;
use crate::ir::{
    Format, Formatter, format_with, group, hard_line, indent, separated, soft_line,
    soft_line_or_space, space, token,
};
use crate::quotes::normalize_quote;
use crate::{CallParentheses, SpaceAfterFunction};

/// Everything after the `function` keyword or name: generics, parameters,
/// return type, block, and `end`.
impl Format for FunctionBody {
    fn fmt(&self, formatter: &mut Formatter) {
        // Luau: `<T, U...>` generic list before the parameter parens.
        self.generics.as_deref().fmt(formatter);

        write_param_list(formatter, self);

        // Luau: `: T` return annotation after `)`.
        if let Some(return_type) = &self.return_type {
            (token(":"), space(), return_type).fmt(formatter);
        }

        if self.block.stmts.is_empty() && self.block.last_stmt.is_none() {
            let anchor = self.block.span.start;
            let end = self.end_keyword_span().start;
            if formatter.comments().has_dangling_comments(anchor, end) {
                // Comments in an otherwise empty body stay indented inside it.
                indent(format_with(move |indented| {
                    hard_line().fmt(indented);
                    write_dangling_comments(indented, anchor, end);
                }))
                .fmt(formatter);
            }
        } else {
            indent((hard_line(), &self.block)).fmt(formatter);
        }
        // Even an empty body puts `end` on its own line.
        (hard_line(), token("end")).fmt(formatter);
    }
}

fn write_param_list(formatter: &mut Formatter, body: &FunctionBody) {
    // The body owns the `(`, so the definition-side `space_after_function_names`
    // space goes here instead of in every caller.
    space_before_definition_paren(formatter);

    let has_params = !body.params.is_empty() || body.vararg.is_some();
    if !has_params {
        token("()").fmt(formatter);
        return;
    }

    let separator = (token(","), soft_line_or_space());
    group((
        token("("),
        indent((
            soft_line(),
            separated(&body.params.items, &separator),
            // The vararg comes last. Unlike a table, a broken parameter list
            // gets no trailing comma, since no dialect allows one.
            body.vararg
                .as_deref()
                .map(|vararg| ((!body.params.is_empty()).then_some(&separator), vararg)),
        )),
        soft_line(),
        token(")"),
    ))
    .fmt(formatter);
}

impl Format for Parameter {
    fn fmt(&self, formatter: &mut Formatter) {
        self.name.fmt(formatter);
        // Luau: `: T` annotation.
        if let Some(type_annotation) = &self.type_annotation {
            (token(":"), space(), type_annotation).fmt(formatter);
        }
    }
}

impl Format for VarArgParam {
    fn fmt(&self, formatter: &mut Formatter) {
        token("...").fmt(formatter);
        // Lua 5.5: `...name`.
        self.name.fmt(formatter);
        // Luau: `: T` annotation (may be a pack, e.g. `...number`).
        if let Some(type_annotation) = &self.type_annotation {
            (token(":"), space(), type_annotation).fmt(formatter);
        }
    }
}

/// A Luau attribute such as `@native` or `@[deprecated("use y")]`. Arguments
/// only exist on the bracketed form.
impl Format for FunctionAttribute {
    fn fmt(&self, formatter: &mut Formatter) {
        match &self.args {
            Some(args) => (
                token("@"),
                token("["),
                &self.name,
                token("("),
                separated(&args.items, (token(","), space())),
                token(")"),
                token("]"),
            )
                .fmt(formatter),
            None => (token("@"), &self.name).fmt(formatter),
        }
    }
}

/// A single link in a method/call chain: optional `:method` accessor + args.
struct ChainSegment<'chain> {
    accessor: Option<&'chain Token>,
    explicit_type_args: Option<&'chain TypeArgs>,
    args: &'chain FunctionArgs,
}

impl Format for ChainSegment<'_> {
    fn fmt(&self, formatter: &mut Formatter) {
        if let Some(name) = self.accessor {
            (token(":"), name).fmt(formatter);
        }
        self.explicit_type_args.map(ExplicitTypeArgs).fmt(formatter);
        self.args.fmt(formatter);
    }
}

/// Collect a left-recursive chain of function calls into (root, segments).
fn collect_call_chain(call: &FunctionCall) -> (&Expression, Vec<ChainSegment<'_>>) {
    let mut segments = Vec::new();
    let mut current = call;
    loop {
        segments.push(ChainSegment {
            accessor: current.method.as_ref(),
            explicit_type_args: current.explicit_type_args.as_deref(),
            args: &current.args,
        });
        let Expression::FunctionCall(inner) = &current.callee else {
            break;
        };
        current = inner;
    }
    segments.reverse();
    (&current.callee, segments)
}

/// A chain worth breaking has at least two method-style (`:`) accessors.
fn is_method_chain(segments: &[ChainSegment<'_>]) -> bool {
    segments.iter().filter(|segment| segment.accessor.is_some()).count() >= 2
}

impl Format for FunctionCall {
    fn fmt(&self, formatter: &mut Formatter) {
        let (root, segments) = collect_call_chain(self);

        ensure_sufficient_stack(|| {
            if is_method_chain(&segments) {
                // Wrap the whole chain so it can break before each accessor.
                group((
                    root,
                    indent(format_with(|indented| {
                        for segment in &segments {
                            if segment.accessor.is_some() {
                                soft_line().fmt(indented);
                            }
                            segment.fmt(indented);
                        }
                    })),
                ))
                .fmt(formatter);
            } else {
                root.fmt(formatter);
                for segment in &segments {
                    segment.fmt(formatter);
                }
            }
        });
    }
}

/// Emits call arguments, adding or dropping the parentheses around a lone
/// string or table as `call_parentheses` asks.
impl Format for FunctionArgs {
    fn fmt(&self, formatter: &mut Formatter) {
        match self {
            Self::Parenthesized { span, args } => {
                // A comment between the parens has to keep them: dropping the
                // parens or hugging a lone argument leaves nowhere to print it.
                if formatter
                    .comments()
                    .has_claimable_comments(*span, args.items.iter().map(Expression::span))
                {
                    space_before_call_paren(formatter);
                    write_commented_list(
                        formatter,
                        ("(", ")"),
                        *span,
                        &args.items,
                        Expression::span,
                        ListSeparator::BetweenItems,
                    );
                    return;
                }

                let can_omit = match formatter.options().call_parentheses {
                    // `Input` preserves the source's choice, which had
                    // parentheses.
                    CallParentheses::Always | CallParentheses::Input => false,
                    CallParentheses::NoSingleString => is_single_string_arg(args),
                    CallParentheses::NoSingleTable => is_single_table_arg(args),
                    CallParentheses::None => {
                        is_single_string_arg(args) || is_single_table_arg(args)
                    }
                };

                if can_omit && let Some(expr) = args.first() {
                    (space(), expr).fmt(formatter);
                } else {
                    space_before_call_paren(formatter);
                    write_args_list(formatter, args);
                }
            }
            Self::TableConstructor(table) => {
                let should_keep_bare = match formatter.options().call_parentheses {
                    CallParentheses::Always | CallParentheses::NoSingleString => false,
                    // `Input` preserves the source's choice, which had none.
                    CallParentheses::NoSingleTable
                    | CallParentheses::None
                    | CallParentheses::Input => true,
                };
                if should_keep_bare {
                    (space(), &**table).fmt(formatter);
                } else {
                    space_before_call_paren(formatter);
                    (token("("), &**table, token(")")).fmt(formatter);
                }
            }
            Self::StringLiteral(literal) => {
                let should_keep_bare = match formatter.options().call_parentheses {
                    CallParentheses::Always | CallParentheses::NoSingleTable => false,
                    // `Input` preserves the source's choice, which had none.
                    CallParentheses::NoSingleString
                    | CallParentheses::None
                    | CallParentheses::Input => true,
                };
                if should_keep_bare {
                    space().fmt(formatter);
                    write_normalized_string(formatter, literal);
                } else {
                    space_before_call_paren(formatter);
                    group((
                        token("("),
                        indent((
                            soft_line(),
                            format_with(|indented| write_normalized_string(indented, literal)),
                        )),
                        soft_line(),
                        token(")"),
                    ))
                    .fmt(formatter);
                }
            }
        }
    }
}

/// Emits a parenthesized argument list, hugging a lone table or function
/// argument as `({...})`.
///
/// No dialect allows a comma after the last argument, so the list never gets
/// a trailing comma and `magic_trailing_comma` only affects tables inside it.
fn write_args_list(formatter: &mut Formatter, args: &Punctuated<Expression>) {
    if args.is_empty() {
        token("()").fmt(formatter);
        return;
    }

    if is_single_huggable_arg(args) {
        (token("("), args.first(), token(")")).fmt(formatter);
        return;
    }

    group((
        token("("),
        indent((soft_line(), separated(&args.items, (token(","), soft_line_or_space())))),
        soft_line(),
        token(")"),
    ))
    .fmt(formatter);
}

fn is_single_string_arg(args: &Punctuated<Expression>) -> bool {
    args.len() == 1 && matches!(args.first(), Some(Expression::StringLiteral(_)))
}

fn is_single_table_arg(args: &Punctuated<Expression>) -> bool {
    args.len() == 1 && matches!(args.first(), Some(Expression::TableConstructor(_)))
}

fn is_single_huggable_arg(args: &Punctuated<Expression>) -> bool {
    args.len() == 1
        && matches!(
            args.first(),
            Some(Expression::TableConstructor(_) | Expression::FunctionDef(_))
        )
}

fn space_before_call_paren(formatter: &mut Formatter) {
    if matches!(
        formatter.options().space_after_function_names,
        SpaceAfterFunction::Calls | SpaceAfterFunction::Always
    ) {
        space().fmt(formatter);
    }
}

fn space_before_definition_paren(formatter: &mut Formatter) {
    if matches!(
        formatter.options().space_after_function_names,
        SpaceAfterFunction::Definitions | SpaceAfterFunction::Always
    ) {
        space().fmt(formatter);
    }
}

/// Re-quotes a string argument written without parentheses, which the AST
/// holds as a bare [`Literal`] instead of an expression.
fn write_normalized_string(formatter: &mut Formatter, literal: &Literal) {
    let normalized = normalize_quote(&literal.text, formatter.options().quote_style);
    formatter.write_text(normalized);
}

#[cfg(test)]
mod tests {
    use luck_ast::{Synth, VarArgParam};
    use luck_token::Span;

    use crate::FormatOptions;
    use crate::fixtures::render;

    #[test]
    fn typed_param() {
        let synth = Synth::new();
        let number = synth.ty_named("number");
        let param = synth.param_typed("n", number);
        assert_eq!(render(&param, FormatOptions::default()), "n: number");
    }

    #[test]
    fn plain_param() {
        let synth = Synth::new();
        let param = synth.param("value");
        assert_eq!(render(&param, FormatOptions::default()), "value");
    }

    #[test]
    fn bare_vararg() {
        let vararg = VarArgParam { span: Span::new(0, 0), name: None, type_annotation: None };
        assert_eq!(render(&vararg, FormatOptions::default()), "...");
    }
}
