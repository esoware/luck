use std::iter;

use luck_ast::{
    Expression, FunctionArgs, FunctionAttribute, FunctionBody, GetSpan as _, Slot, TypedName,
    VarArgParam,
};
use luck_token::{Span, TokenKind};

use crate::format_table::TablePlacement;
use crate::ir::{Formatter, LineMode};
use crate::{CallParentheses, SpaceAfterFunction};

const fn is_single_string_arg(args: &[Expression<'_>]) -> bool {
    matches!(args, [Expression::StringLiteral(_)])
}

const fn is_single_table_arg(args: &[Expression<'_>]) -> bool {
    matches!(args, [Expression::TableConstructor(_)])
}

fn is_block_arg(arg: &Expression<'_>) -> bool {
    matches!(arg, Expression::FunctionDef(_))
        || matches!(arg, Expression::TableConstructor(table) if !table.fields.is_empty())
}

impl<'ast> Formatter<'ast> {
    pub(crate) fn write_function_body(&mut self, body: &FunctionBody<'ast>) {
        // `<T, U...>` generic list before the parameter parens.
        if let Some(generics) = &body.generics {
            self.write_generic_type_list(generics);
        }

        self.write_param_list(body);

        // `: T` return annotation after `)`.
        if let Some(return_type) = &body.return_type {
            self.token(":");
            self.space();
            self.write_type(return_type);
        }

        if self.has_printable_content(&body.block) {
            self.indent(|f| {
                f.hard_line();
                f.write_block(&body.block);
            });
            self.hard_line();
        } else {
            self.space();
        }
        self.token("end");
    }

    fn write_param_list(&mut self, body: &FunctionBody<'ast>) {
        // The body owns the `(`, so the definition-side
        // `space_after_function_names` space goes here instead of in every
        // caller.
        if matches!(
            self.options().space_after_function_names,
            SpaceAfterFunction::Definitions | SpaceAfterFunction::Always
        ) {
            self.space();
        }

        // The tree keeps no span for the parentheses.
        let open = body.generics.as_ref().map_or(body.span.start, |generics| generics.span.end);
        let last_end = body
            .vararg
            .as_ref()
            .map(|vararg| vararg.span.end)
            .or_else(|| body.params.last().map(|param| param.span().end));
        let closer = self.closer_offset(last_end.unwrap_or(open), TokenKind::RightParen);
        if last_end.is_none() {
            self.write_dangling_list(("(", ")"), closer, LineMode::Soft);
            return;
        }

        self.group(|grouped| {
            grouped.token("(");
            grouped.indent(|f| {
                f.soft_line();
                for (i, param) in body.params.iter().enumerate() {
                    if i > 0 {
                        f.token(",");
                        f.soft_line_or_space();
                    }
                    f.write_typed_name(param);
                }
                // The vararg comes last. Unlike a table, a broken parameter
                // list gets no trailing comma, since the grammar allows none.
                if let Some(vararg) = &body.vararg {
                    if !body.params.is_empty() {
                        f.token(",");
                        f.soft_line_or_space();
                    }
                    f.write_vararg_param(vararg);
                }
                f.write_closing_comments(closer);
            });
            grouped.soft_line();
            grouped.token(")");
        });
    }

    pub(crate) fn write_typed_name(&mut self, typed_name: &TypedName<'ast>) {
        self.node(
            |_| typed_name.span(),
            |f| {
                f.name(typed_name.name);
                if let Some(annotation) = &typed_name.type_annotation {
                    f.token(":");
                    f.space();
                    f.write_type(annotation);
                }
            },
        );
    }

    fn write_vararg_param(&mut self, vararg: &VarArgParam<'ast>) {
        self.node(
            |_| vararg.span,
            |f| {
                f.token("...");
                // `: T` annotation (may be a pack, e.g. `...number`).
                if let Some(type_annotation) = &vararg.type_annotation {
                    f.token(":");
                    f.space();
                    f.write_type(type_annotation);
                }
            },
        );
    }

    /// Arguments only exist on the bracketed form.
    pub(crate) fn write_function_attribute(&mut self, attribute: &FunctionAttribute<'ast>) {
        self.node(
            |_| attribute.span,
            |f| {
                f.token("@");
                let Some(args) = &attribute.args else {
                    f.name(attribute.name);
                    return;
                };
                f.token("[");
                f.name(attribute.name);
                if args.is_empty() {
                    f.write_dangling_list(("(", ")"), attribute.span.end, LineMode::Soft);
                } else {
                    f.token("(");
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            f.token(",");
                            f.space();
                        }
                        f.write_expr(arg);
                    }
                    f.write_closing_comments(attribute.span.end);
                    f.token(")");
                }
                f.token("]");
            },
        );
    }

    /// Nothing inside the arguments takes a comment past their end, which
    /// trails the call.
    pub(crate) fn write_function_args(&mut self, function_args: &FunctionArgs<'ast>) {
        self.inside(function_args.span().end, |f| f.write_args_inside(function_args));
    }

    fn write_args_inside(&mut self, function_args: &FunctionArgs<'ast>) {
        let call_parentheses = self.options().call_parentheses;
        match function_args {
            FunctionArgs::Parenthesized { span, args } => {
                // A comment between the parens and the lone argument would
                // have nowhere to go without them.
                let can_omit = match call_parentheses {
                    // `Input` preserves the source's choice, which had
                    // parentheses.
                    CallParentheses::Always | CallParentheses::Input => false,
                    CallParentheses::NoSingleString => is_single_string_arg(args),
                    CallParentheses::NoSingleTable => is_single_table_arg(args),
                    CallParentheses::None => {
                        is_single_string_arg(args) || is_single_table_arg(args)
                    }
                } && !self
                    .comments()
                    .has_comment_outside(*span, args.iter().map(Expression::span));

                if can_omit && let Some(expr) = args.first() {
                    self.space();
                    self.write_expr(expr);
                } else {
                    self.space_before_call_paren();
                    self.write_args_list(args, *span);
                }
            }
            FunctionArgs::TableConstructor(table) => {
                let should_keep_bare = match call_parentheses {
                    CallParentheses::Always | CallParentheses::NoSingleString => false,
                    // `Input` preserves the source's choice, which had none.
                    CallParentheses::NoSingleTable
                    | CallParentheses::None
                    | CallParentheses::Input => true,
                };
                let write_table = |f: &mut Self| {
                    f.node(
                        |_| table.span,
                        |inner| inner.write_table(table, TablePlacement::Standalone),
                    );
                };
                if should_keep_bare {
                    self.space();
                    write_table(self);
                } else if self.comments().has_line_ending_comment_before(table.span.start) {
                    // As the parenthesized table reads back, where such a
                    // comment between the parens keeps them off the table.
                    self.space_before_call_paren();
                    let group_id = self.group_id();
                    self.group_with_id(group_id, |grouped| {
                        grouped.token("(");
                        grouped.indent_if_group_breaks(group_id, |f| {
                            f.soft_line();
                            write_table(f);
                        });
                        grouped.soft_line();
                        grouped.token(")");
                    });
                } else {
                    self.space_before_call_paren();
                    self.token("(");
                    write_table(self);
                    self.token(")");
                }
            }
            FunctionArgs::StringLiteral(literal) => {
                let should_keep_bare = match call_parentheses {
                    CallParentheses::Always | CallParentheses::NoSingleTable => false,
                    // `Input` preserves the source's choice, which had none.
                    CallParentheses::NoSingleString
                    | CallParentheses::None
                    | CallParentheses::Input => true,
                };
                let write_string = |f: &mut Self| {
                    f.node(|_| literal.span, |inner| inner.write_string(literal));
                };
                if should_keep_bare {
                    self.space();
                    write_string(self);
                } else {
                    self.space_before_call_paren();
                    self.group(|grouped| {
                        grouped.token("(");
                        grouped.indent(|f| {
                            f.soft_line();
                            write_string(f);
                        });
                        grouped.soft_line();
                        grouped.token(")");
                    });
                }
            }
            // A parse-recovery placeholder has no text to print.
            FunctionArgs::Error(_) => {}
        }
    }

    /// The grammar allows no comma after the last argument, so the list never
    /// gets a trailing comma and `magic_trailing_comma` only affects tables
    /// inside it.
    ///
    /// The arguments that are functions or tables with fields are hugged: a
    /// list too long for one line keeps the other arguments beside the parens
    /// and breaks inside those, as in `f(a, function()` or `f(a, {` ... `}, {`.
    /// When another argument sits between two of them, when a comment outside
    /// them ends its line, or when that layout doesn't fit, every argument
    /// gets its own line.
    fn write_args_list(&mut self, args: &[Expression<'ast>], span: Span) {
        match args {
            [] => {
                self.write_dangling_list(("(", ")"), span.end, LineMode::Soft);
                return;
            }
            [arg @ (Expression::TableConstructor(_) | Expression::FunctionDef(_))]
                if !self
                    .comments()
                    .has_line_ending_comment_outside(span, iter::once(arg.span())) =>
            {
                self.token("(");
                self.write_expr(arg);
                self.write_closing_comments(span.end);
                self.token(")");
                return;
            }
            [_, ..] => {}
        }

        let has_split_blocks = args
            .iter()
            .map(is_block_arg)
            .skip_while(|is_block| !is_block)
            .skip_while(|is_block| *is_block)
            .any(|is_block| is_block);
        let can_hug = !has_split_blocks
            && args.iter().any(is_block_arg)
            && !self.comments().has_line_ending_comment_outside(
                span,
                args.iter().filter(|arg| is_block_arg(arg)).map(Expression::span),
            );

        let group_id = self.group_id();
        self.group_with_id(group_id, |grouped| {
            grouped.token("(");
            grouped.indent_if_group_breaks(group_id, |f| {
                f.soft_line();
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        f.token(",");
                        f.soft_line_or_space();
                    }
                    let is_hugged = can_hug && is_block_arg(arg);
                    if is_hugged
                        && let Expression::TableConstructor(table) = arg
                        && !f.is_wrapped(arg, Slot::FREE)
                    {
                        f.node(
                            |outer| outer.extent(arg),
                            |inner| inner.write_table(table, TablePlacement::Hugged),
                        );
                    } else if is_hugged {
                        let function_group = f.group_id();
                        f.hugged_group_with_id(function_group, |hugged| hugged.write_expr(arg));
                    } else {
                        f.write_expr(arg);
                    }
                }
                f.write_closing_comments(span.end);
            });
            grouped.soft_line();
            grouped.token(")");
        });
    }

    fn space_before_call_paren(&mut self) {
        if matches!(
            self.options().space_after_function_names,
            SpaceAfterFunction::Calls | SpaceAfterFunction::Always
        ) {
            self.space();
        }
    }
}
