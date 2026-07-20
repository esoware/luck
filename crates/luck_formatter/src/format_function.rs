//! Function bodies, calls, and their argument/parameter lists.
//!
//! [`FormatFunctionBody`] is the shared entry the statement- and
//! expression-level function emitters call into; the call/argument layout
//! ports the old formatter's hugging and parameter-breaking heuristics onto
//! the combinator IR.

use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Literal};
use luck_ast::shared::{FunctionBody, Parameter, Punctuated, VarArgParam};
use luck_token::Token;

use crate::ir::*;
use crate::tokens::FormatToken;
use crate::{CallParentheses, SpaceAfterFunction};

/// A function body without the leading `function` keyword or name: generics,
/// parameter parens, optional return type, block, and `end`.
///
/// The name and shape are a pinned contract - the statement and expression
/// emitters construct this and call [`Format::fmt`].
pub(crate) struct FormatFunctionBody<'a> {
    pub body: &'a FunctionBody,
}

impl Format for FormatFunctionBody<'_> {
    fn fmt(&self, f: &mut Formatter) {
        let body = self.body;

        // Luau: `<T, U...>` generic list before the parameter parens.
        if let Some(generics) = &body.generics {
            generics.fmt(f);
        }

        write_param_list(f, body);

        // Luau: `: T` return annotation after `)`.
        if let Some(return_type) = &body.return_type {
            token(":").fmt(f);
            space().fmt(f);
            return_type.fmt(f);
        }

        let is_empty = body.block.stmts.is_empty() && body.block.last_stmt.is_none();
        let has_dangling_comments = f
            .comments
            .has_dangling_comments(body.block.span.start, body.end_keyword_span().start);
        if is_empty && has_dangling_comments {
            // A body that is empty of statements but holds comments keeps
            // them indented inside, not relocated after the function.
            let anchor = body.block.span.start;
            let end = body.end_keyword_span().start;
            indent(format_with(move |f| {
                hard_line().fmt(f);
                f.emit_dangling_comments(anchor, end);
            }))
            .fmt(f);
            hard_line().fmt(f);
            token("end").fmt(f);
        } else if is_empty {
            // An empty body still puts `end` on its own line, matching the old
            // layout (`function()` then `end`) rather than an indented blank.
            hard_line().fmt(f);
            token("end").fmt(f);
        } else {
            indent((hard_line(), &body.block)).fmt(f);
            hard_line().fmt(f);
            token("end").fmt(f);
        }
    }
}

fn write_param_list(f: &mut Formatter, body: &FunctionBody) {
    // The definition-side space (`space_after_function_names`) sits between the
    // name/keyword and the parameter parens. `FormatFunctionBody` owns the `(`,
    // so it emits the space here instead of relying on every caller.
    space_before_def_paren(f);

    let has_params = !body.params.is_empty() || body.vararg.is_some();
    if !has_params {
        token("()").fmt(f);
        return;
    }

    group(format_with(move |f| {
        token("(").fmt(f);
        indent(format_with(move |f| {
            soft_line().fmt(f);
            let mut is_first = true;
            for param in body.params.items.iter() {
                if !is_first {
                    token(",").fmt(f);
                    soft_line_or_space().fmt(f);
                }
                is_first = false;
                param.fmt(f);
            }
            // Vararg is always last. No trailing comma, ever: Luau's parlist
            // grammar forbids one after `...`, so emitting it breaks re-parse.
            if let Some(vararg) = &body.vararg {
                if !is_first {
                    token(",").fmt(f);
                    soft_line_or_space().fmt(f);
                }
                vararg.fmt(f);
            }
        }))
        .fmt(f);
        soft_line().fmt(f);
        token(")").fmt(f);
    }))
    .fmt(f);
}

impl Format for Parameter {
    fn fmt(&self, f: &mut Formatter) {
        FormatToken(&self.name).fmt(f);
        // Luau: `: T` annotation.
        if let Some(type_annotation) = &self.type_annotation {
            token(":").fmt(f);
            space().fmt(f);
            type_annotation.fmt(f);
        }
    }
}

impl Format for VarArgParam {
    fn fmt(&self, f: &mut Formatter) {
        token("...").fmt(f);
        // Lua 5.5: `...name`.
        if let Some(name) = &self.name {
            FormatToken(name).fmt(f);
        }
        // Luau: `: T` annotation (may be a pack, e.g. `...number`).
        if let Some(type_annotation) = &self.type_annotation {
            token(":").fmt(f);
            space().fmt(f);
            type_annotation.fmt(f);
        }
    }
}

/// A single link in a method/call chain: optional `:method` accessor + args.
struct ChainSegment<'a> {
    accessor: Option<&'a Token>,
    args: &'a FunctionArgs,
}

/// Collect a left-recursive chain of function calls into (root, segments).
fn collect_chain(call: &FunctionCall) -> (&Expression, Vec<ChainSegment<'_>>) {
    let mut segments = Vec::new();
    let mut current = call;
    loop {
        segments.push(ChainSegment {
            accessor: current.method.as_ref(),
            args: &current.args,
        });
        match &current.callee {
            Expression::FunctionCall(inner) => current = inner,
            _ => break,
        }
    }
    segments.reverse();
    (&current.callee, segments)
}

/// A chain worth breaking has at least two method-style (`:`) accessors.
fn is_method_chain(segments: &[ChainSegment]) -> bool {
    segments
        .iter()
        .filter(|segment| segment.accessor.is_some())
        .count()
        >= 2
}

impl Format for FunctionCall {
    fn fmt(&self, f: &mut Formatter) {
        let (root, segments) = collect_chain(self);

        if is_method_chain(&segments) {
            // Wrap the whole chain so it can break before each accessor.
            group(format_with(|f| {
                root.fmt(f);
                indent(format_with(|f| {
                    for segment in &segments {
                        if let Some(name) = segment.accessor {
                            soft_line().fmt(f);
                            token(":").fmt(f);
                            FormatToken(name).fmt(f);
                        }
                        segment.args.fmt(f);
                    }
                }))
                .fmt(f);
            }))
            .fmt(f);
        } else {
            root.fmt(f);
            for segment in &segments {
                if let Some(name) = segment.accessor {
                    token(":").fmt(f);
                    FormatToken(name).fmt(f);
                }
                segment.args.fmt(f);
            }
        }
    }
}

impl Format for FunctionArgs {
    fn fmt(&self, f: &mut Formatter) {
        write_call_args(f, self);
    }
}

/// Emit call arguments, honoring `call_parentheses` sugar (bare string/table
/// arguments) before falling back to a parenthesized, breakable list.
fn write_call_args(f: &mut Formatter, args: &FunctionArgs) {
    match args {
        FunctionArgs::Parenthesized { args, .. } => {
            let can_omit = match f.options.call_parentheses {
                CallParentheses::Always => false,
                CallParentheses::NoSingleString => is_single_string_arg(args),
                CallParentheses::NoSingleTable => is_single_table_arg(args),
                CallParentheses::None => is_single_string_arg(args) || is_single_table_arg(args),
                // Source had parentheses - preserve that choice.
                CallParentheses::Input => false,
            };

            if can_omit {
                let expr = args.first().expect("single-arg check guarantees an item");
                match expr {
                    Expression::StringLiteral(literal) => {
                        space().fmt(f);
                        write_normalized_string(f, literal);
                    }
                    Expression::TableConstructor(_) => {
                        space().fmt(f);
                        expr.fmt(f);
                    }
                    _ => {
                        space_before_call_paren(f);
                        write_args_list(f, args);
                    }
                }
            } else {
                space_before_call_paren(f);
                write_args_list(f, args);
            }
        }
        FunctionArgs::TableConstructor(table) => {
            // AST carries a bare table arg (no parens). Add parens unless the
            // option allows bare.
            let keep_bare = match f.options.call_parentheses {
                CallParentheses::Always => false,
                CallParentheses::NoSingleTable | CallParentheses::None => true,
                CallParentheses::NoSingleString => false,
                // Source already had no parens - preserve that choice.
                CallParentheses::Input => true,
            };
            if keep_bare {
                space().fmt(f);
                table.fmt(f);
            } else {
                space_before_call_paren(f);
                token("(").fmt(f);
                table.fmt(f);
                token(")").fmt(f);
            }
        }
        FunctionArgs::StringLiteral(literal) => {
            // AST carries a bare string arg (no parens). Add parens unless the
            // option allows bare.
            let keep_bare = match f.options.call_parentheses {
                CallParentheses::Always => false,
                CallParentheses::NoSingleString | CallParentheses::None => true,
                CallParentheses::NoSingleTable => false,
                // Source already had no parens - preserve that choice.
                CallParentheses::Input => true,
            };
            if keep_bare {
                space().fmt(f);
                write_normalized_string(f, literal);
            } else {
                space_before_call_paren(f);
                group(format_with(move |f| {
                    token("(").fmt(f);
                    indent(format_with(move |f| {
                        soft_line().fmt(f);
                        write_normalized_string(f, literal);
                    }))
                    .fmt(f);
                    soft_line().fmt(f);
                    token(")").fmt(f);
                }))
                .fmt(f);
            }
        }
    }
}

/// Emit a parenthesized argument list. A single table/function argument is
/// "hugged" - `({...})` rather than an indented break - unless a magic
/// trailing comma requests a forced multi-line layout.
fn write_args_list(f: &mut Formatter, args: &Punctuated<Expression>) {
    if args.is_empty() {
        token("()").fmt(f);
        return;
    }

    let force_expand = f.options.magic_trailing_comma && args.has_trailing_separator;

    if is_single_huggable_arg(args) && !force_expand {
        token("(").fmt(f);
        args.first().expect("hug check guarantees an item").fmt(f);
        token(")").fmt(f);
        return;
    }

    group(format_with(move |f| {
        token("(").fmt(f);
        indent(format_with(move |f| {
            soft_line().fmt(f);
            for (index, expr) in args.items.iter().enumerate() {
                if index > 0 {
                    soft_line_or_space().fmt(f);
                }
                expr.fmt(f);
                if index + 1 < args.items.len() || args.has_trailing_separator {
                    token(",").fmt(f);
                }
            }
            if force_expand {
                expand_parent().fmt(f);
            }
        }))
        .fmt(f);
        soft_line().fmt(f);
        token(")").fmt(f);
    }))
    .fmt(f);
}

fn is_single_string_arg(args: &Punctuated<Expression>) -> bool {
    args.len() == 1 && matches!(args.first(), Some(Expression::StringLiteral(_)))
}

fn is_single_table_arg(args: &Punctuated<Expression>) -> bool {
    args.len() == 1 && matches!(args.first(), Some(Expression::TableConstructor(_)))
}

/// A single table or function argument gets `({...})` instead of an indented
/// break.
fn is_single_huggable_arg(args: &Punctuated<Expression>) -> bool {
    args.len() == 1
        && matches!(
            args.first(),
            Some(Expression::TableConstructor(_)) | Some(Expression::FunctionDef(_))
        )
}

fn space_before_call_paren(f: &mut Formatter) {
    if matches!(
        f.options.space_after_function_names,
        SpaceAfterFunction::Calls | SpaceAfterFunction::Always
    ) {
        space().fmt(f);
    }
}

fn space_before_def_paren(f: &mut Formatter) {
    if matches!(
        f.options.space_after_function_names,
        SpaceAfterFunction::Definitions | SpaceAfterFunction::Always
    ) {
        space().fmt(f);
    }
}

/// Emit a string literal with quotes normalized to the configured style. Bare
/// and hugged string arguments format their own leaf here; parenthesized
/// arguments defer to the expression impl.
fn write_normalized_string(f: &mut Formatter, literal: &Literal) {
    let normalized = crate::quotes::normalize_quote(&literal.text, f.options.quote_style);
    text(normalized).fmt(f);
}

#[cfg(test)]
mod tests {
    use luck_ast::shared::{Parameter, VarArgParam};
    use luck_ast::synth::Synth;
    use luck_token::Span;

    use crate::ir::{Format, Formatter};
    use crate::printer::{PrinterOptions, print};

    fn render(node: &dyn Format) -> String {
        let mut formatter = Formatter::new();
        node.fmt(&mut formatter);
        let group_count = formatter.group_count();
        let elements = formatter.into_elements();
        print(
            &elements,
            group_count,
            &PrinterOptions {
                line_width: 80,
                use_tabs: false,
                indent_width: 2,
            },
        )
    }

    #[test]
    fn typed_parameter() {
        let synth = Synth::new();
        let number = synth.ty_named("number");
        let param: Parameter = synth.param_typed("n", number);
        assert_eq!(render(&param), "n: number");
    }

    #[test]
    fn plain_parameter() {
        let synth = Synth::new();
        let param: Parameter = synth.param("value");
        assert_eq!(render(&param), "value");
    }

    #[test]
    fn bare_vararg() {
        let vararg = VarArgParam {
            span: Span::new(0, 0),
            name: None,
            type_annotation: None,
        };
        assert_eq!(render(&vararg), "...");
    }
}
