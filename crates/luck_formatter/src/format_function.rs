use luck_ast::{Expression, FunctionArgs, FunctionAttribute, FunctionBody, Parameter, VarArgParam};

use crate::comments::ListKind;
use crate::ir::Formatter;
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

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the formatter is one type with its node families split by file"
)]
impl<'ast> Formatter<'ast> {
    pub(crate) fn write_function_body(&mut self, body: &FunctionBody<'ast>) {
        // Luau: `<T, U...>` generic list before the parameter parens.
        if let Some(generics) = &body.generics {
            self.write_generic_type_list(generics);
        }

        self.write_param_list(body);

        // Luau: `: T` return annotation after `)`.
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

        if body.params.is_empty() && body.vararg.is_none() {
            self.token("()");
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
                    f.write_parameter(param);
                }
                // The vararg comes last. Unlike a table, a broken parameter
                // list gets no trailing comma, since no dialect allows one.
                if let Some(vararg) = &body.vararg {
                    if !body.params.is_empty() {
                        f.token(",");
                        f.soft_line_or_space();
                    }
                    f.write_vararg_param(vararg);
                }
            });
            grouped.soft_line();
            grouped.token(")");
        });
    }

    pub(crate) fn write_parameter(&mut self, param: &Parameter<'ast>) {
        self.text(param.name.text);
        // Luau: `: T` annotation.
        if let Some(type_annotation) = &param.type_annotation {
            self.token(":");
            self.space();
            self.write_type(type_annotation);
        }
    }

    fn write_vararg_param(&mut self, vararg: &VarArgParam<'ast>) {
        self.token("...");
        // Lua 5.5: `...name`.
        if let Some(name) = &vararg.name {
            self.text(name.text);
        }
        // Luau: `: T` annotation (may be a pack, e.g. `...number`).
        if let Some(type_annotation) = &vararg.type_annotation {
            self.token(":");
            self.space();
            self.write_type(type_annotation);
        }
    }

    /// Arguments only exist on the bracketed form.
    pub(crate) fn write_function_attribute(&mut self, attribute: &FunctionAttribute<'ast>) {
        self.token("@");
        let Some(args) = &attribute.args else {
            self.text(attribute.name.text);
            return;
        };
        self.token("[");
        self.text(attribute.name.text);
        self.token("(");
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.token(",");
                self.space();
            }
            self.write_expr(arg);
        }
        self.token(")");
        self.token("]");
    }

    pub(crate) fn write_function_args(&mut self, function_args: &FunctionArgs<'ast>) {
        let call_parentheses = self.options().call_parentheses;
        match function_args {
            FunctionArgs::Parenthesized { span, args } => {
                // A comment between the parens has to keep them: dropping the
                // parens or hugging a lone argument leaves nowhere to print it.
                if self.comments().has_claimable_comments(*span, args.iter().map(Expression::span))
                {
                    self.space_before_call_paren();
                    self.write_commented_list(
                        ("(", ")"),
                        *span,
                        args,
                        ListKind::CallArgs,
                        Expression::span,
                        Self::write_expr,
                    );
                    return;
                }

                let can_omit = match call_parentheses {
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
                    self.space();
                    self.write_expr(expr);
                } else {
                    self.space_before_call_paren();
                    self.write_args_list(args);
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
                if should_keep_bare {
                    self.space();
                    self.write_table(table, false);
                } else {
                    self.space_before_call_paren();
                    self.token("(");
                    self.write_table(table, false);
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
                if should_keep_bare {
                    self.space();
                    self.write_string(literal.text);
                } else {
                    self.space_before_call_paren();
                    self.group(|grouped| {
                        grouped.token("(");
                        grouped.indent(|f| {
                            f.soft_line();
                            f.write_string(literal.text);
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

    /// No dialect allows a comma after the last argument, so the list never
    /// gets a trailing comma and `magic_trailing_comma` only affects tables
    /// inside it.
    ///
    /// The arguments that are functions or tables with fields are hugged: a
    /// list too long for one line keeps the other arguments beside the parens
    /// and breaks inside those, as in `f(a, function()` or `f(a, {` ... `}, {`.
    /// When another argument sits between two of them, or when that layout
    /// doesn't fit, every argument gets its own line.
    fn write_args_list(&mut self, args: &[Expression<'ast>]) {
        match args {
            [] => {
                self.token("()");
                return;
            }
            [arg @ (Expression::TableConstructor(_) | Expression::FunctionDef(_))] => {
                self.token("(");
                self.write_expr(arg);
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
                    let is_hugged = !has_split_blocks && is_block_arg(arg);
                    if is_hugged && let Expression::TableConstructor(table) = arg {
                        f.write_table(table, true);
                    } else if is_hugged {
                        let function_group = f.group_id();
                        f.hugged_group_with_id(function_group, |hugged| hugged.write_expr(arg));
                    } else {
                        f.write_expr(arg);
                    }
                }
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
