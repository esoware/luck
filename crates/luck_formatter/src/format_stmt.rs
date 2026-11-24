//! Block statements mostly share the shape `keyword`, an indented block, a
//! hard line, and `end`.
//!
//! The comments between statements are placed in `format_block`, and the
//! ones inside a statement by the nodes it writes.

use std::{iter, slice};

use luck_ast::{
    Assignment, Attribute, AttributedName, Block, CompoundAssignment, Expression,
    FunctionAttribute, FunctionName, GenericFor, GetSpan as _, IfStatement, LocalAssignment,
    LocalFunctionKind, NumericFor, ReturnStatement, Slot, Statement, Type, TypeDeclaration,
    TypeDeclarationValue,
};

use crate::CollapseSimpleStatement;
use crate::format_block::first_item_start;
use crate::ir::Formatter;

/// What follows `=`.
///
/// A lone table, function, or call breaks inside its own brackets, so it stays
/// on the `=` line. Anything else that doesn't fit there goes on its own
/// indented line when it fits that, and hangs from `=` otherwise.
fn is_bracketed(items: &[Expression<'_>]) -> bool {
    matches!(items, [Expression::TableConstructor(_) | Expression::FunctionDef(_)])
        || matches!(items, [Expression::Suffixed(chain)] if chain.is_call())
}

impl<'ast> Formatter<'ast> {
    pub(crate) fn write_stmt(&mut self, stmt: &Statement<'ast>) {
        match stmt {
            Statement::LocalAssignment(local) => self.write_local_assignment(local),
            Statement::Assignment(assignment) => self.write_assignment(assignment),
            Statement::FunctionCall(call) => self.write_suffixed(call),
            Statement::FunctionDecl(function_decl) => {
                self.write_function_attributes(
                    &function_decl.attributes,
                    function_decl.name.span.start,
                );
                self.token("function");
                self.space();
                self.write_function_name(&function_decl.name);
                self.write_function_body(&function_decl.body);
            }
            Statement::LocalFunction(local_function) => {
                let name = local_function.name;
                self.write_function_attributes(&local_function.attributes, name.span.start);
                self.token(match local_function.kind {
                    LocalFunctionKind::Local => "local",
                    LocalFunctionKind::Const => "const",
                    LocalFunctionKind::Export => "export",
                });
                self.space();
                self.token("function");
                self.space();
                self.name(name);
                self.write_function_body(&local_function.body);
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.token("global");
                self.space();
                self.token("function");
                self.space();
                self.name(global_function.name);
                self.write_function_body(&global_function.body);
            }
            Statement::DoBlock(do_block) => {
                self.token("do");
                self.write_body_then_end(&do_block.block);
            }
            Statement::WhileLoop(while_loop) => {
                self.write_header("while", &while_loop.condition, "do");
                self.write_body_then_end(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                // `until <cond>` closes the loop, so no `end` and no
                // line-suffix flush.
                self.token("repeat");
                self.write_indented_block(&repeat_loop.block);
                self.hard_line();
                self.token("until");
                self.space();
                self.write_condition(&repeat_loop.condition, true);
            }
            Statement::IfStatement(if_stmt) => self.write_if(if_stmt),
            Statement::NumericFor(numeric_for) => self.write_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.write_generic_for(generic_for),
            Statement::Return(return_stmt) => self.write_return(return_stmt),
            Statement::Break(_) => self.token("break"),
            // Luau
            Statement::Continue(_) => self.token("continue"),
            // Lua 5.2+
            Statement::Goto(goto) => {
                self.token("goto");
                self.space();
                self.name(goto.name);
            }
            // Lua 5.2+
            Statement::Label(label) => {
                self.token("::");
                self.name(label.name);
                self.token("::");
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                self.token("global");
                self.space();
                if let Some(attribute) = &global.attribute {
                    self.write_attribute(attribute);
                    self.space();
                }
                self.write_attributed_names_inline(&global.names);
                if !global.exprs.is_empty() {
                    self.space();
                    self.token("=");
                    self.space();
                    self.write_assigned_values(&global.exprs);
                }
            }
            // Lua 5.5
            Statement::GlobalStar(global_star) => {
                self.token("global");
                self.space();
                if let Some(attribute) = &global_star.attribute {
                    self.write_attribute(attribute);
                    self.space();
                }
                self.token("*");
            }
            // Luau
            Statement::CompoundAssignment(compound) => self.write_compound_assignment(compound),
            // Luau
            Statement::TypeDeclaration(declaration) => self.write_type_declaration(declaration),
            // A parse-recovery placeholder has no text to print.
            Statement::Error(_) => {}
        }
    }

    fn write_return(&mut self, return_stmt: &ReturnStatement<'ast>) {
        let Some(first) = return_stmt.exprs.first() else {
            self.token("return");
            return;
        };
        self.group(|grouped| {
            grouped.token("return");
            grouped.space();
            grouped.write_after_operator(first.span().start, |f| {
                f.write_expr_list(&return_stmt.exprs);
            });
        });
    }

    /// The block, then `end` on a line of its own.
    fn write_body_then_end(&mut self, block: &Block<'ast>) {
        self.write_indented_block(block);
        self.hard_line();
        self.token("end");
    }

    /// `if cond then`, `elseif cond then`, or `while cond do`, a group so a
    /// long condition can break onto its own indented line before `then` or
    /// `do`.
    fn write_header(
        &mut self,
        open: &'static str,
        condition: &Expression<'ast>,
        close: &'static str,
    ) {
        self.group(|grouped| {
            grouped.token(open);
            grouped.indent(|f| {
                f.soft_line_or_space();
                f.write_condition(condition, false);
            });
            grouped.soft_line_or_space();
            grouped.token(close);
        });
    }

    /// Statement conditions are boolean contexts, so the parentheses around a
    /// whole one come off, even the ones the tree keeps: `(f())` truncating
    /// values to one changes nothing observable. They stay around a comment,
    /// which needs the line breaks they hold.
    ///
    /// An operator chain breaks level with its first operand unless
    /// `is_indented`, for a condition that sits on an indented line of its own.
    fn write_condition(&mut self, condition: &Expression<'ast>, is_indented: bool) {
        let unwrapped = condition.unwrap_parentheses();
        let outer = self.grouping_around(condition.span()).unwrap_or_else(|| condition.span());
        if self.comments().has_comment_outside(outer, iter::once(unwrapped.span())) {
            self.write_expr(condition);
        } else if let Expression::BinaryOp(chain) = unwrapped
            && !is_indented
        {
            self.node(|| outer, |f| f.write_binary_chain(chain, false, Slot::FREE));
        } else {
            self.node(|| outer, |f| f.write_bare_expr(unwrapped, Slot::FREE));
        }
    }

    /// Writes `content`, which follows an operator or a keyword such as `=` or
    /// `in`, on the next line, one indent deeper, when a comment leading it
    /// ends its line, as Prettier breaks an assignment after its `=`. The
    /// printer trims the space written before it from the line's end.
    pub(crate) fn write_after_operator(&mut self, start: u32, content: impl FnOnce(&mut Self)) {
        if self.comments().has_line_ending_comment_before(start) {
            self.indent(|f| {
                f.hard_line();
                content(f);
            });
        } else {
            content(self);
        }
    }

    /// Two or more share one indented group so they wrap together. A lone item
    /// breaks on its own.
    fn write_expr_list(&mut self, exprs: &[Expression<'ast>]) {
        let write = |f: &mut Self| {
            for (i, expr) in exprs.iter().enumerate() {
                if i > 0 {
                    f.token(",");
                    f.soft_line_or_space();
                }
                f.write_expr(expr);
            }
        };
        if exprs.len() > 1 {
            self.group(|f| f.indent(write));
        } else {
            write(self);
        }
    }

    /// A value a statement assigns. Only a function written here keeps the
    /// require sorting of the block around it.
    fn write_assigned_value(&mut self, value: &Expression<'ast>) {
        if let Expression::FunctionDef(function_def) = value
            && !self.is_wrapped(value, Slot::FREE)
        {
            let span = self.extent(value);
            self.node(|| span, |f| f.write_function_definition(function_def));
        } else {
            self.write_expr(value);
        }
    }

    /// The values a statement assigns, which print as an expression list does.
    fn write_assigned_values(&mut self, values: &[Expression<'ast>]) {
        let Some(first) = values.first() else {
            return;
        };
        let write = |f: &mut Self| {
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    f.token(",");
                    f.soft_line_or_space();
                }
                f.write_assigned_value(value);
            }
        };
        let write_list = |f: &mut Self| {
            if values.len() > 1 {
                f.group(|grouped| grouped.indent(write));
            } else {
                write(f);
            }
        };
        self.write_after_operator(self.extent(first).start, |f| {
            if is_bracketed(values) {
                write_list(f);
            } else {
                f.next_line_group(write_list);
            }
        });
    }

    /// Never broken onto several lines.
    fn write_attributed_names_inline(&mut self, names: &[AttributedName<'ast>]) {
        for (i, name) in names.iter().enumerate() {
            if i > 0 {
                self.token(",");
                self.space();
            }
            self.write_attributed_name(name);
        }
    }

    fn write_attributed_name(&mut self, name: &AttributedName<'ast>) {
        self.node(
            || name.span(),
            |f| {
                f.name(name.name);
                if let Some(annotation) = &name.type_annotation {
                    f.token(":");
                    f.space();
                    f.write_type(annotation);
                }
                if let Some(attribute) = &name.attribute {
                    f.space();
                    f.write_attribute(attribute);
                }
            },
        );
    }

    fn write_attribute(&mut self, attribute: &Attribute<'ast>) {
        self.node(
            || attribute.span,
            |f| {
                f.token("<");
                f.name(attribute.name);
                f.token(">");
            },
        );
    }

    fn write_local_assignment(&mut self, local: &LocalAssignment<'ast>) {
        if local.kind.is_exported() {
            self.token("export");
            self.space();
        }
        self.token(if local.kind.is_const() { "const" } else { "local" });
        self.space();
        if let Some(attribute) = &local.attribute {
            self.write_attribute(attribute);
            self.space();
        }
        // Names get their own group so a long binding list breaks
        // independently of the value list.
        self.group(|f| {
            for (i, name) in local.names.iter().enumerate() {
                if i > 0 {
                    f.token(",");
                    f.soft_line_or_space();
                }
                f.write_attributed_name(name);
            }
        });
        if !local.exprs.is_empty() {
            self.space();
            self.token("=");
            self.space();
            self.write_assigned_values(&local.exprs);
        }
    }

    fn write_assignment(&mut self, assignment: &Assignment<'ast>) {
        // Targets are never broken onto multiple lines.
        self.group(|f| {
            for (i, target) in assignment.targets.iter().enumerate() {
                if i > 0 {
                    f.token(",");
                    f.space();
                }
                f.write_var(target);
            }
            f.space();
            f.token("=");
            f.space();
            f.write_assigned_values(&assignment.values);
        });
    }

    fn write_compound_assignment(&mut self, compound: &CompoundAssignment<'ast>) {
        self.group(|grouped| {
            grouped.write_var(&compound.var);
            grouped.space();
            grouped.token(compound.op.static_text());
            grouped.space();
            grouped.write_assigned_values(slice::from_ref(&compound.expr));
        });
    }

    fn write_if(&mut self, if_stmt: &IfStatement<'ast>) {
        // Collapses to `if cond then stmt end` when the option allows, there's
        // one branch holding one statement, and the whole `if` fits on one
        // line. A comment that breaks a line inside it breaks the group.
        let can_collapse = self.options().collapse_simple_statement
            == CollapseSimpleStatement::ConditionalOnly
            && if_stmt.elseif_clauses.is_empty()
            && if_stmt.else_clause.is_none()
            && if_stmt.block.stmts.len() == 1
            && first_item_start(&if_stmt.block).is_some();

        if can_collapse {
            // The group stays flat only if the whole `if` fits, and a body
            // spanning lines forces it to break.
            self.group(|grouped| {
                grouped.write_header("if", &if_stmt.condition, "then");
                grouped.indent(|f| {
                    f.soft_line_or_space();
                    f.write_block(&if_stmt.block);
                });
                grouped.soft_line_or_space();
                grouped.token("end");
            });
            return;
        }

        self.write_header("if", &if_stmt.condition, "then");
        self.write_indented_block(&if_stmt.block);

        for clause in &if_stmt.elseif_clauses {
            self.hard_line();
            self.write_header("elseif", &clause.condition, "then");
            self.write_indented_block(&clause.block);
        }

        if let Some(else_clause) = &if_stmt.else_clause {
            self.hard_line();
            self.token("else");
            self.write_indented_block(&else_clause.block);
        }

        self.hard_line();
        self.token("end");
    }

    fn write_numeric_for(&mut self, numeric_for: &NumericFor<'ast>) {
        self.token("for");
        self.space();
        self.name(numeric_for.name);
        // Luau: `: T` on the loop variable.
        if let Some(annotation) = &numeric_for.type_annotation {
            self.token(":");
            self.space();
            self.write_type(annotation);
        }
        self.space();
        self.token("=");
        self.space();
        self.write_after_operator(numeric_for.start.span().start, |f| {
            f.write_expr(&numeric_for.start);
            f.token(",");
            f.space();
            f.write_expr(&numeric_for.limit);
            if let Some(step) = &numeric_for.step {
                f.token(",");
                f.space();
                f.write_expr(step);
            }
        });
        self.space();
        self.token("do");
        self.write_body_then_end(&numeric_for.block);
    }

    fn write_generic_for(&mut self, generic_for: &GenericFor<'ast>) {
        // The bindings are never broken onto multiple lines.
        self.token("for");
        self.space();
        for (i, name) in generic_for.names.iter().enumerate() {
            if i > 0 {
                self.token(",");
                self.space();
            }
            self.write_parameter(name);
        }
        self.space();
        self.token("in");
        self.space();
        if let Some(first) = generic_for.exprs.first() {
            self.write_after_operator(first.span().start, |f| {
                f.write_expr_list(&generic_for.exprs);
            });
        }
        self.space();
        self.token("do");
        self.write_body_then_end(&generic_for.block);
    }

    /// Each on a line of its own. The comments between the last one and the
    /// declaration's name go above the keywords.
    fn write_function_attributes(
        &mut self,
        attributes: &[FunctionAttribute<'ast>],
        name_start: u32,
    ) {
        if attributes.is_empty() {
            return;
        }
        for attribute in attributes {
            self.write_function_attribute(attribute);
            self.hard_line();
        }
        self.write_leading_comments(name_start);
    }

    fn write_function_name(&mut self, name: &FunctionName<'ast>) {
        self.node(
            || name.span,
            |f| {
                for (i, part) in name.names.iter().enumerate() {
                    if i > 0 {
                        f.token(".");
                    }
                    f.name(*part);
                }
                if let Some(method_name) = &name.method {
                    f.token(":");
                    f.name(*method_name);
                }
            },
        );
    }

    fn write_type_declaration(&mut self, declaration: &TypeDeclaration<'ast>) {
        if declaration.is_exported {
            self.token("export");
            self.space();
        }
        self.token("type");
        match &declaration.value {
            TypeDeclarationValue::Alias(annotation) => {
                self.space();
                self.name(declaration.name);
                if let Some(generics) = &declaration.generics {
                    self.write_generic_type_list(generics);
                }
                self.space();
                self.token("=");
                self.space();
                // An alternation breaks after the `=` itself.
                if matches!(annotation, Type::Union(_) | Type::Intersection(_)) {
                    self.write_type(annotation);
                } else {
                    self.write_after_operator(annotation.span().start, |f| {
                        f.write_type(annotation);
                    });
                }
            }
            TypeDeclarationValue::TypeFunction(body) => {
                self.space();
                self.token("function");
                self.space();
                self.name(declaration.name);
                self.without_sorting_requires(|f| f.write_function_body(body));
            }
        }
    }
}
