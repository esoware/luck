use luck_ast::{
    Expression, FunctionArgs, FunctionAttribute, FunctionBody, FunctionCall, Literal, Parameter,
    Punctuated, TypeArgs, VarArgParam, ensure_sufficient_stack,
};
use luck_token::Token;

use crate::comments::{ListKind, write_commented_list};
use crate::format_block::has_printable_content;
use crate::format_table::HuggedTable;
use crate::format_type::ExplicitTypeArgs;
use crate::ir::{
    Format, Formatter, flat, format_with, group, group_with_id, hard_line, hugged_group_with_id,
    indent, indent_if_group_breaks, separated, soft_line, soft_line_or_space, space, token,
};
use crate::quotes::normalize_quote;
use crate::{CallParentheses, SpaceAfterFunction};

impl Format for FunctionBody {
    fn fmt(&self, f: &mut Formatter) {
        // Luau: `<T, U...>` generic list before the parameter parens.
        self.generics.as_deref().fmt(f);

        write_param_list(f, self);

        // Luau: `: T` return annotation after `)`.
        if let Some(return_type) = &self.return_type {
            (token(":"), space(), return_type).fmt(f);
        }

        if has_printable_content(f, &self.block) {
            (indent((hard_line(), &self.block)), hard_line(), token("end")).fmt(f);
        } else {
            (space(), token("end")).fmt(f);
        }
    }
}

fn write_param_list(f: &mut Formatter, body: &FunctionBody) {
    // The body owns the `(`, so the definition-side `space_after_function_names`
    // space goes here instead of in every caller.
    space_before_definition_paren(f);

    let has_params = !body.params.is_empty() || body.vararg.is_some();
    if !has_params {
        token("()").fmt(f);
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
    .fmt(f);
}

impl Format for Parameter {
    fn fmt(&self, f: &mut Formatter) {
        self.name.fmt(f);
        // Luau: `: T` annotation.
        if let Some(type_annotation) = &self.type_annotation {
            (token(":"), space(), type_annotation).fmt(f);
        }
    }
}

impl Format for VarArgParam {
    fn fmt(&self, f: &mut Formatter) {
        token("...").fmt(f);
        // Lua 5.5: `...name`.
        self.name.fmt(f);
        // Luau: `: T` annotation (may be a pack, e.g. `...number`).
        if let Some(type_annotation) = &self.type_annotation {
            (token(":"), space(), type_annotation).fmt(f);
        }
    }
}

/// Arguments only exist on the bracketed form.
impl Format for FunctionAttribute {
    fn fmt(&self, f: &mut Formatter) {
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
                .fmt(f),
            None => (token("@"), &self.name).fmt(f),
        }
    }
}

struct ChainSegment<'chain> {
    accessor: Option<&'chain Token>,
    explicit_type_args: Option<&'chain TypeArgs>,
    args: &'chain FunctionArgs,
}

impl Format for ChainSegment<'_> {
    fn fmt(&self, f: &mut Formatter) {
        let type_args = self.explicit_type_args.map(ExplicitTypeArgs);
        if let Some(name) = self.accessor {
            (token(":"), name).fmt(f);
            // Luau rejects a method call whose `(` is on a later line than
            // its name.
            flat(type_args).fmt(f);
        } else {
            type_args.fmt(f);
        }
        self.args.fmt(f);
    }
}

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
    fn fmt(&self, f: &mut Formatter) {
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
                .fmt(f);
            } else {
                root.fmt(f);
                for segment in &segments {
                    segment.fmt(f);
                }
            }
        });
    }
}

impl Format for FunctionArgs {
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Parenthesized { span, args } => {
                // A comment between the parens has to keep them: dropping the
                // parens or hugging a lone argument leaves nowhere to print it.
                if f.comments()
                    .has_claimable_comments(*span, args.items.iter().map(Expression::span))
                {
                    space_before_call_paren(f);
                    write_commented_list(
                        f,
                        ("(", ")"),
                        *span,
                        &args.items,
                        Expression::span,
                        ListKind::CallArgs,
                    );
                    return;
                }

                let can_omit = match f.options().call_parentheses {
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
                    (space(), expr).fmt(f);
                } else {
                    space_before_call_paren(f);
                    write_args_list(f, args);
                }
            }
            Self::TableConstructor(table) => {
                let should_keep_bare = match f.options().call_parentheses {
                    CallParentheses::Always | CallParentheses::NoSingleString => false,
                    // `Input` preserves the source's choice, which had none.
                    CallParentheses::NoSingleTable
                    | CallParentheses::None
                    | CallParentheses::Input => true,
                };
                if should_keep_bare {
                    (space(), &**table).fmt(f);
                } else {
                    space_before_call_paren(f);
                    (token("("), &**table, token(")")).fmt(f);
                }
            }
            Self::StringLiteral(literal) => {
                let should_keep_bare = match f.options().call_parentheses {
                    CallParentheses::Always | CallParentheses::NoSingleTable => false,
                    // `Input` preserves the source's choice, which had none.
                    CallParentheses::NoSingleString
                    | CallParentheses::None
                    | CallParentheses::Input => true,
                };
                if should_keep_bare {
                    space().fmt(f);
                    write_normalized_string(f, literal);
                } else {
                    space_before_call_paren(f);
                    group((
                        token("("),
                        indent((
                            soft_line(),
                            format_with(|indented| write_normalized_string(indented, literal)),
                        )),
                        soft_line(),
                        token(")"),
                    ))
                    .fmt(f);
                }
            }
        }
    }
}

/// No dialect allows a comma after the last argument, so the list never gets
/// a trailing comma and `magic_trailing_comma` only affects tables inside it.
///
/// The arguments that are functions or tables with fields are hugged: a list
/// too long for one line keeps the other arguments beside the parens and
/// breaks inside those, as in `f(a, function()` or `f(a, {` ... `}, {`.
/// When another argument sits between two of them, or when that layout
/// doesn't fit, every argument gets its own line.
fn write_args_list(f: &mut Formatter, args: &Punctuated<Expression>) {
    if args.is_empty() {
        token("()").fmt(f);
        return;
    }

    if is_single_huggable_arg(args) {
        (token("("), args.first(), token(")")).fmt(f);
        return;
    }

    let is_block_arg = |arg: &Expression| {
        matches!(arg, Expression::FunctionDef(_))
            || matches!(arg, Expression::TableConstructor(table) if !table.fields.is_empty())
    };
    let has_split_blocks = args
        .iter()
        .map(is_block_arg)
        .skip_while(|is_block| !is_block)
        .skip_while(|is_block| *is_block)
        .any(|is_block| is_block);

    let group_id = f.group_id();
    group_with_id(
        group_id,
        (
            token("("),
            indent_if_group_breaks(
                group_id,
                (
                    soft_line(),
                    format_with(|list| {
                        for (i, arg) in args.iter().enumerate() {
                            if i > 0 {
                                (token(","), soft_line_or_space()).fmt(list);
                            }
                            let is_hugged = !has_split_blocks && is_block_arg(arg);
                            if is_hugged && let Expression::TableConstructor(table) = arg {
                                HuggedTable(table).fmt(list);
                            } else if is_hugged {
                                let function_group = list.group_id();
                                hugged_group_with_id(function_group, arg).fmt(list);
                            } else {
                                arg.fmt(list);
                            }
                        }
                    }),
                ),
            ),
            soft_line(),
            token(")"),
        ),
    )
    .fmt(f);
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

fn space_before_call_paren(f: &mut Formatter) {
    if matches!(
        f.options().space_after_function_names,
        SpaceAfterFunction::Calls | SpaceAfterFunction::Always
    ) {
        space().fmt(f);
    }
}

fn space_before_definition_paren(f: &mut Formatter) {
    if matches!(
        f.options().space_after_function_names,
        SpaceAfterFunction::Definitions | SpaceAfterFunction::Always
    ) {
        space().fmt(f);
    }
}

/// Re-quotes a string argument written without parentheses, which the AST
/// holds as a bare [`Literal`] instead of an expression.
fn write_normalized_string(f: &mut Formatter, literal: &Literal) {
    let normalized = normalize_quote(&literal.text, f.options().quote_style);
    f.write_text(normalized);
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
